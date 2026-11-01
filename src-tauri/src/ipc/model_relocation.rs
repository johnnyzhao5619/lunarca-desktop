//! 模型存储搬移（PRD-10-014，TDD-04 §2.3.3）：把 Lunarca 受管模型根搬到另一目录 / 另一块盘。
//!
//! 四段式「拷贝 → 校验 → 切换 → 删旧」，**绝不边移边删**：任何中断点上旧根都完好且仍是
//! 唯一有效位置；只有全部文件按 SHA-256 逐件校验通过后才写 `storage.models_root`，
//! 之后才删旧根。取消 / 失败只清理新根的暂存。跨卷 rename 非原子，故不用 rename。
//!
//! 边界：
//! - 只搬 LunaManaged 资产（整个模型根：whisper 权重 + CoreML 工件 + 随包引擎的 GGUF 条目 +
//!   说话人分离模型）。
//! - 搬移期间拒绝新的模型安装；有安装进行中时拒绝开始搬移（同一目录不能同时被两件事写）。
//! - 切换后进程内已加载的 whisper context 一律作废，下次任务按新路径重载（`evict_asr_caches`
//!   + `refresh_asr_assembly`）；说话人分离 Provider 按新根重登记；随包引擎按新路径重编排。
//! - 计划是确认快照：开始前重新预检一次（目录、空间、安装态），确认期间世界变了就拒绝。

use super::*;
use sha2::Digest as _;

/// 目标卷预留：搬完至少还剩这么多（与安装预检同一口径）。
const RELOCATION_FREE_SPACE_RESERVE: u64 = 512 * 1024 * 1024;
/// 拷贝块大小；每块之间检查取消。
const COPY_CHUNK: usize = 1024 * 1024;

/// 搬移计划（确认快照）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct StorageRelocationPlanDto {
    pub plan_id: String,
    pub from: String,
    pub to: String,
    /// 要搬的字节数。
    pub bytes: f64,
    /// 目标卷此刻可用字节；探不到为 `None`。
    pub available_bytes: Option<f64>,
    pub can_start: bool,
    /// 不能开始的稳定码（前端据此取文案）。
    pub blocking_reason: Option<String>,
}

/// 搬移进度事件（`models://relocation`）。`phase` 终态：`done` / `failed` / `cancelled`。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "models://relocation")]
pub struct StorageRelocationProgress {
    pub phase: String,
    pub copied_bytes: f64,
    pub total_bytes: f64,
    pub to: String,
    pub error: Option<String>,
}

/// 运行态（单例）。
#[derive(Default)]
pub(crate) struct RelocationState {
    plans: HashMap<String, StorageRelocationPlanDto>,
    running: Option<(luna_core::CancelToken, StorageRelocationProgress)>,
    last: Option<StorageRelocationProgress>,
}

impl RelocationState {
    /// 是否有一次搬移在进行（安装登记据此拒绝新安装）。
    pub(crate) fn is_running(&self) -> bool {
        self.running.is_some()
    }
}

fn dir_total_bytes(path: &Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.file_type().is_symlink() {
            continue;
        }
        total += if meta.is_dir() {
            dir_total_bytes(&entry.path())?
        } else {
            meta.len()
        };
    }
    Ok(total)
}

fn available_space_for(path: &Path) -> Option<u64> {
    let mut candidate = path;
    while !candidate.exists() {
        candidate = candidate.parent()?;
    }
    fs4::available_space(candidate).ok()
}

/// 目标目录必须不存在或为空目录，且可写（建目录 + 写探针文件）。
fn probe_target_writable(to: &Path) -> Result<(), String> {
    if to.exists() {
        if !to.is_dir() {
            return Err("RELOCATION_TARGET_NOT_DIR".to_owned());
        }
        let mut entries =
            std::fs::read_dir(to).map_err(|e| format!("RELOCATION_TARGET_NOT_WRITABLE: {e}"))?;
        if entries.next().is_some() {
            return Err("RELOCATION_TARGET_NOT_EMPTY".to_owned());
        }
    } else {
        std::fs::create_dir_all(to).map_err(|e| format!("RELOCATION_TARGET_NOT_WRITABLE: {e}"))?;
    }
    let probe = to.join(".luna-relocation-probe");
    std::fs::write(&probe, b"probe").map_err(|e| format!("RELOCATION_TARGET_NOT_WRITABLE: {e}"))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// 规范化路径：目标可能还不存在（用户在对话框里新建了文件夹又删了、或指向待建目录），
/// 于是取**最深的已存在祖先**做 `canonicalize`，再把剩下的段接回去——否则 `/var/...`
/// 与 `/private/var/...` 这种符号链接差异会让「互相包含」判空（macOS 临时目录正是如此）。
fn canonical_or_partial(path: &Path) -> PathBuf {
    let mut existing = path;
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name.to_os_string());
                existing = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
    let mut out = std::fs::canonicalize(existing).unwrap_or_else(|_| existing.to_path_buf());
    for seg in rest.iter().rev() {
        out.push(seg);
    }
    out
}

/// 纯预检（不碰磁盘之外的状态）：同目录 / 互相包含 / 目标非空或不可写 / 空间不足。
pub(crate) fn preflight(from: &Path, to: &Path) -> Result<(u64, Option<u64>), String> {
    if !to.is_absolute() {
        return Err("RELOCATION_TARGET_NOT_ABSOLUTE".to_owned());
    }
    let from_canon = canonical_or_partial(from);
    let to_canon = canonical_or_partial(to);
    if from_canon == to_canon {
        return Err("RELOCATION_SAME_DIR".to_owned());
    }
    if to_canon.starts_with(&from_canon) || from_canon.starts_with(&to_canon) {
        return Err("RELOCATION_NESTED".to_owned());
    }
    let bytes = dir_total_bytes(from).map_err(|e| format!("RELOCATION_SOURCE_UNREADABLE: {e}"))?;
    probe_target_writable(to)?;
    let available = available_space_for(to);
    if let Some(avail) = available {
        if avail < bytes.saturating_add(RELOCATION_FREE_SPACE_RESERVE) {
            return Err("RELOCATION_INSUFFICIENT_DISK".to_owned());
        }
    }
    Ok((bytes, available))
}

fn plan_from(
    from: &Path,
    to: &Path,
    outcome: Result<(u64, Option<u64>), String>,
) -> StorageRelocationPlanDto {
    let (bytes, available, reason) = match outcome {
        Ok((bytes, available)) => (bytes, available, None),
        Err(code) => (
            dir_total_bytes(from).unwrap_or(0),
            available_space_for(to),
            Some(code),
        ),
    };
    StorageRelocationPlanDto {
        plan_id: uuid::Uuid::now_v7().to_string(),
        from: from.to_string_lossy().into_owned(),
        to: to.to_string_lossy().into_owned(),
        bytes: bytes as f64,
        available_bytes: available.map(|b| b as f64),
        can_start: reason.is_none(),
        blocking_reason: reason,
    }
}

/// 只读预检 + 登记计划。`new_root` 由前端经目录选择器取得（原生对话框），此处不信任它。
#[tauri::command]
#[specta::specta]
pub(crate) async fn plan_storage_relocation(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    new_root: String,
) -> Result<StorageRelocationPlanDto, String> {
    let from = state.models_root(&app);
    let to = PathBuf::from(new_root);
    let busy = {
        let reloc = state
            .storage_relocation
            .lock()
            .map_err(|_| "搬移锁中毒".to_string())?;
        reloc.running.is_some()
    };
    let installing = state
        .model_installs
        .lock()
        .map(|m| !m.is_empty())
        .unwrap_or(false);
    let outcome = if busy {
        Err("RELOCATION_ALREADY_RUNNING".to_owned())
    } else if installing {
        Err("RELOCATION_INSTALL_IN_PROGRESS".to_owned())
    } else {
        let (from2, to2) = (from.clone(), to.clone());
        tauri::async_runtime::spawn_blocking(move || preflight(&from2, &to2))
            .await
            .map_err(|e| format!("RELOCATION_PREFLIGHT_FAILED: {e}"))?
    };
    let plan = plan_from(&from, &to, outcome);
    if plan.can_start {
        let mut reloc = state
            .storage_relocation
            .lock()
            .map_err(|_| "搬移锁中毒".to_string())?;
        reloc.plans.clear();
        reloc.plans.insert(plan.plan_id.clone(), plan.clone());
    }
    Ok(plan)
}

/// 开始搬移：重新预检 → 后台四段式 → 进度事件。返回即已启动。
#[tauri::command]
#[specta::specta]
pub(crate) async fn start_storage_relocation(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    plan_id: String,
) -> Result<(), String> {
    let plan = {
        let mut reloc = state
            .storage_relocation
            .lock()
            .map_err(|_| "搬移锁中毒".to_string())?;
        if reloc.running.is_some() {
            return Err("RELOCATION_ALREADY_RUNNING".to_owned());
        }
        reloc
            .plans
            .remove(&plan_id)
            .ok_or_else(|| "RELOCATION_PLAN_NOT_FOUND".to_owned())?
    };
    let from = state.models_root(&app);
    let to = PathBuf::from(&plan.to);
    if from.to_string_lossy() != plan.from {
        return Err("RELOCATION_PLAN_STALE".to_owned());
    }
    if state
        .model_installs
        .lock()
        .map(|m| !m.is_empty())
        .unwrap_or(false)
    {
        return Err("RELOCATION_INSTALL_IN_PROGRESS".to_owned());
    }
    // 确认期间世界可能变了：再检一次（目标被占用、空间被吃掉）。
    {
        let (from2, to2) = (from.clone(), to.clone());
        tauri::async_runtime::spawn_blocking(move || preflight(&from2, &to2))
            .await
            .map_err(|e| format!("RELOCATION_PREFLIGHT_FAILED: {e}"))??;
    }
    let cancel = luna_core::CancelToken::new();
    let initial = StorageRelocationProgress {
        phase: "copying".to_owned(),
        copied_bytes: 0.0,
        total_bytes: plan.bytes,
        to: plan.to.clone(),
        error: None,
    };
    {
        let mut reloc = state
            .storage_relocation
            .lock()
            .map_err(|_| "搬移锁中毒".to_string())?;
        reloc.running = Some((cancel.clone(), initial.clone()));
    }
    let _ = initial.emit(&app);
    let total = plan.bytes as u64;
    tauri::async_runtime::spawn_blocking(move || {
        run_relocation(app, from, to, total, cancel);
    });
    Ok(())
}

/// 取消：拷贝 / 校验阶段即时生效；已进入切换阶段则不可取消（那一步只是写一个键）。
#[tauri::command]
#[specta::specta]
pub(crate) fn cancel_storage_relocation(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let reloc = state
        .storage_relocation
        .lock()
        .map_err(|_| "搬移锁中毒".to_string())?;
    if let Some((cancel, _)) = reloc.running.as_ref() {
        cancel.cancel();
    }
    Ok(())
}

/// 重挂载恢复：正在跑 → 最新进度；跑完 → 终态一帧；从没跑过 → `None`。
#[tauri::command]
#[specta::specta]
pub(crate) fn storage_relocation_status(
    state: tauri::State<'_, AppState>,
) -> Result<Option<StorageRelocationProgress>, String> {
    let reloc = state
        .storage_relocation
        .lock()
        .map_err(|_| "搬移锁中毒".to_string())?;
    Ok(reloc
        .running
        .as_ref()
        .map(|(_, p)| p.clone())
        .or_else(|| reloc.last.clone()))
}

fn publish(app: &tauri::AppHandle, progress: StorageRelocationProgress) {
    // 阶段转移打一行日志（不含笔记内容，只有阶段与字节数）：真机上「卡在哪一步」全靠它。
    if progress.phase != "copying" {
        eprintln!(
            "[relocation] phase={} copied={} total={} error={:?}",
            progress.phase, progress.copied_bytes, progress.total_bytes, progress.error
        );
    }
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(mut reloc) = state.storage_relocation.lock() {
            let terminal = matches!(progress.phase.as_str(), "done" | "failed" | "cancelled");
            if terminal {
                reloc.running = None;
                reloc.last = Some(progress.clone());
            } else if let Some((_, last)) = reloc.running.as_mut() {
                *last = progress.clone();
            }
        }
    }
    let _ = progress.emit(app);
}

/// 递归列出相对文件路径（跳过符号链接）。
fn list_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.file_type().is_symlink() {
            continue;
        }
        let path = entry.path();
        if meta.is_dir() {
            list_files(root, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_path_buf());
        }
    }
    Ok(())
}

fn sha256_of(path: &Path, cancel: &luna_core::CancelToken) -> std::io::Result<Option<[u8; 32]>> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = vec![0u8; COPY_CHUNK];
    loop {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(Some(hasher.finalize().into()))
}

/// 分块拷贝一个文件，边拷边报进度；取消返回 `Ok(false)`。
fn copy_file(
    src: &Path,
    dst: &Path,
    cancel: &luna_core::CancelToken,
    copied: &mut u64,
    mut on_progress: impl FnMut(u64),
) -> std::io::Result<bool> {
    use std::io::{Read as _, Write as _};
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut input = std::fs::File::open(src)?;
    let mut output = std::fs::File::create(dst)?;
    let mut buf = vec![0u8; COPY_CHUNK];
    loop {
        if cancel.is_cancelled() {
            return Ok(false);
        }
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        output.write_all(&buf[..n])?;
        *copied += n as u64;
        on_progress(*copied);
    }
    output.sync_all()?;
    Ok(true)
}

/// 四段式主体（阻塞线程）。终态一律经 `publish` 收口。
fn run_relocation(
    app: tauri::AppHandle,
    from: PathBuf,
    to: PathBuf,
    total: u64,
    cancel: luna_core::CancelToken,
) {
    let to_text = to.to_string_lossy().into_owned();
    let frame = |phase: &str, copied: u64, error: Option<String>| StorageRelocationProgress {
        phase: phase.to_owned(),
        copied_bytes: copied as f64,
        total_bytes: total as f64,
        to: to_text.clone(),
        error,
    };
    let abort = |app: &tauri::AppHandle, phase: &str, copied: u64, error: Option<String>| {
        // 只清新根暂存：旧根从未被动过。
        let _ = std::fs::remove_dir_all(&to);
        publish(app, frame(phase, copied, error));
    };

    let mut files = Vec::new();
    if let Err(e) = list_files(&from, &from, &mut files) {
        abort(
            &app,
            "failed",
            0,
            Some(format!("RELOCATION_SOURCE_UNREADABLE: {e}")),
        );
        return;
    }
    // 清单里有 `.part` = 有下载正在写这个目录（预检与开始之间挤进来的）：拷一半的权重
    // 搬过去是坏文件，不如现在就停。
    if files
        .iter()
        .any(|rel| rel.extension().is_some_and(|ext| ext == "part"))
    {
        abort(
            &app,
            "failed",
            0,
            Some("RELOCATION_INSTALL_IN_PROGRESS".to_owned()),
        );
        return;
    }

    // ① 拷贝。
    let mut copied = 0u64;
    let mut last_report = 0u64;
    for rel in &files {
        let (src, dst) = (from.join(rel), to.join(rel));
        let result = copy_file(&src, &dst, &cancel, &mut copied, |done| {
            if done.saturating_sub(last_report) >= 8 * 1024 * 1024 {
                last_report = done;
                publish(&app, frame("copying", done, None));
            }
        });
        match result {
            Ok(true) => {}
            Ok(false) => {
                abort(&app, "cancelled", copied, None);
                return;
            }
            Err(e) => {
                abort(
                    &app,
                    "failed",
                    copied,
                    Some(format!("RELOCATION_COPY_FAILED: {e}")),
                );
                return;
            }
        }
    }

    // ② 校验：逐件 SHA-256 对比，任何一件不一致即失败（旧根仍完好）。
    publish(&app, frame("verifying", copied, None));
    for rel in &files {
        let (src, dst) = (from.join(rel), to.join(rel));
        match (sha256_of(&src, &cancel), sha256_of(&dst, &cancel)) {
            (Ok(Some(a)), Ok(Some(b))) if a == b => {}
            (Ok(None), _) | (_, Ok(None)) => {
                abort(&app, "cancelled", copied, None);
                return;
            }
            (Ok(_), Ok(_)) => {
                abort(
                    &app,
                    "failed",
                    copied,
                    Some(format!("RELOCATION_VERIFY_MISMATCH: {}", rel.display())),
                );
                return;
            }
            (Err(e), _) | (_, Err(e)) => {
                abort(
                    &app,
                    "failed",
                    copied,
                    Some(format!("RELOCATION_VERIFY_FAILED: {e}")),
                );
                return;
            }
        }
    }

    // ③ 切换：写设备级设置（唯一写入点），作废进程内已加载的权重，按新根重装配。
    publish(&app, frame("switching", copied, None));
    let Some(state) = app.try_state::<AppState>() else {
        abort(
            &app,
            "failed",
            copied,
            Some("RELOCATION_SWITCH_FAILED: no app state".to_owned()),
        );
        return;
    };
    let switched = (|| -> Result<(), String> {
        let mut guard = state
            .settings
            .lock()
            .map_err(|_| "Settings 锁中毒".to_string())?;
        let mut next = guard.clone();
        next.set("storage.models_root", to_text.clone());
        persist_device_settings(&next, &state.settings_persistence)?;
        *guard = next;
        Ok(())
    })();
    if let Err(e) = switched {
        abort(
            &app,
            "failed",
            copied,
            Some(format!("RELOCATION_SWITCH_FAILED: {e}")),
        );
        return;
    }
    state.evict_asr_caches();
    let _ = state.refresh_asr_assembly(&app);
    state.register_diarize_provider_if_available(&app);
    // 随包引擎的 preset 写的是权重的绝对路径：按新根重编排（路径变了即重启路由器），
    // 且必须在删旧之前——否则路由器下一次按需加载会去旧目录找一个已删掉的文件。
    // 本函数跑在阻塞线程上（见签名注），故 `block_on` 而不是 `.await`。
    tauri::async_runtime::block_on(crate::local_engine::reconcile(&app, false));
    let _ = SettingsChanged {
        key: "storage.models_root".to_owned(),
    }
    .emit(&app);

    // ④ 删旧：切换已生效，这一步失败只是多占一份磁盘，如实告知但不算搬移失败。
    publish(&app, frame("cleaning", copied, None));
    let cleanup_error = std::fs::remove_dir_all(&from)
        .err()
        .map(|e| format!("RELOCATION_OLD_ROOT_NOT_REMOVED: {e}"));
    publish(&app, frame("done", copied, cleanup_error));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_rejects_same_nested_and_non_empty_targets() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("models");
        std::fs::create_dir_all(from.join("whisper-x")).unwrap();
        std::fs::write(from.join("whisper-x/weights.bin"), [1u8; 64]).unwrap();

        assert_eq!(preflight(&from, &from).unwrap_err(), "RELOCATION_SAME_DIR");
        assert_eq!(
            preflight(&from, &from.join("inner")).unwrap_err(),
            "RELOCATION_NESTED"
        );
        assert_eq!(
            preflight(&from, tmp.path()).unwrap_err(),
            "RELOCATION_NESTED",
            "目标包含源目录同样是嵌套"
        );
        let busy = tmp.path().join("busy");
        std::fs::create_dir_all(&busy).unwrap();
        std::fs::write(busy.join("x"), b"x").unwrap();
        assert_eq!(
            preflight(&from, &busy).unwrap_err(),
            "RELOCATION_TARGET_NOT_EMPTY"
        );
        assert_eq!(
            preflight(&from, Path::new("relative/dir")).unwrap_err(),
            "RELOCATION_TARGET_NOT_ABSOLUTE"
        );

        let fresh = tmp.path().join("fresh");
        let (bytes, _) = preflight(&from, &fresh).unwrap();
        assert_eq!(bytes, 64);
        assert!(fresh.is_dir(), "预检会建好目标目录");
        assert!(
            !fresh.join(".luna-relocation-probe").exists(),
            "探针文件不留"
        );
    }

    #[test]
    fn copy_and_verify_helpers_round_trip_and_honour_cancel() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("a.bin");
        let dst = tmp.path().join("out/a.bin");
        std::fs::write(&src, vec![7u8; 3 * 1024 * 1024 + 11]).unwrap();
        let cancel = luna_core::CancelToken::new();
        let mut copied = 0;
        assert!(copy_file(&src, &dst, &cancel, &mut copied, |_| {}).unwrap());
        assert_eq!(copied, 3 * 1024 * 1024 + 11);
        assert_eq!(
            sha256_of(&src, &cancel).unwrap(),
            sha256_of(&dst, &cancel).unwrap()
        );
        cancel.cancel();
        let mut again = 0;
        assert!(!copy_file(&src, &tmp.path().join("b.bin"), &cancel, &mut again, |_| {}).unwrap());
        assert!(sha256_of(&src, &cancel).unwrap().is_none());
    }
}
