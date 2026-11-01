//! 库内文件操作命令域（PRD-06，IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 职责边界：新建/移动/重命名/删除、快照与回收站、AI 产物落盘、以及本地媒体服务
//! 的资产解析。**每一条都会改动用户的文件**，故一律经 `Vault` 而非直接碰文件系统。
//!
//! 跨模块可见性：命令与本域 DTO `pub(crate)`；共享 helper 经 `use super::*` 从 `mod.rs` 取。

use super::*;

/// 经模板创建一篇 Vault 笔记。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_create(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
    note_type: NoteTypeDto,
    body: String,
) -> Result<NoteDocDto, String> {
    let change_scope = VaultChangeScope::for_window(window.label());
    // 窗口感知（PRD-06-050）：副库窗口的 wiki 幽灵创建 / 图谱幽灵节点落它自己的库。
    let doc: NoteDocDto = state.with_vault(window.label(), |vault| {
        vault
            .create(
                &path,
                note_type.into(),
                &TemplateVars {
                    body,
                    ..TemplateVars::default()
                },
            )
            .map(Into::into)
            .map_err(vault_err)
    })?;
    emit_vault_changed_in_scope(&app, &doc.path, false, change_scope);
    Ok(doc)
}

/// 把若干库内对象移动进某个目录（拖拽移动 / 「移动到 ▸」，PRD-06-006 / PRD-07-062）。
///
/// `target_dir` 为空串 = Vault 根。落点文件名由 Rust 按各自的末级名拼接——**前端不拼路径**
/// （ADR-003）：它此前要自己算 `目标目录/末级名`，于是同一段拼接散在文件树、四条清单的
/// 「移动到」与文件夹改名三处，各写了一遍。
///
/// **整批先验、再动手**：越界、把目录移进自身、落点已被占用、**同一批里两项重名**
/// 一律在动手前查完并整批拒绝——它们都是**这次选择本身**就能判定的错，让用户改选择比
/// 让他面对一个搬了一半的库好。已在落点目录里的项按"已达成"跳过，不算错也不重复搬。
///
/// **预检之外的失败逐项交代，不中止整批**：磁盘满、权限、外部改动这些预检看不见的
/// 原因只属于**那一项**，为它整批停下会留下"前三项搬走了、后两项还在原处"且没人说得清
/// 是哪几项的半途状态（2026-09-02 真机：拖四个成功三个，界面只字未提）。故返回值分两栏：
/// 成功的 `moved`（界面据它重映射标签与选中项）与失败的 `failed`（逐项带原因）。
///
/// **广播在锁外**：见 `emit_vault_changes`——锁内逐条广播会让派生刷新全部排在自己
/// 手里这把锁后面，把整个界面拖死。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_move_into(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
    target_dir: String,
) -> Result<VaultMoveResultDto, String> {
    let target_dir = target_dir.trim_end_matches('/').to_owned();
    let mut result = VaultMoveResultDto::default();
    let mut metric_moves: Vec<(String, String)> = Vec::new();
    let mut relinked: Vec<String> = Vec::new();
    let mut receipt = RelocationReceiptDto::default();
    let (root, changes) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let root = vault.root().to_path_buf();

        let plan =
            plan_moves(&paths, &target_dir, |path| vault.exists(path)).map_err(|rejection| {
                match rejection {
                    MovePlanRejection::IntoItself => {
                        coded_error(&app, "INVALID_MOVE", "invalid-move-into-self")
                    }
                    MovePlanRejection::TargetOccupied => {
                        coded_error(&app, "ALREADY_EXISTS", "move-target-exists")
                    }
                }
            })?;
        if plan.is_empty() {
            return Ok(result);
        }

        // 手工链接度量的路径重映射：整批只扫一次库（每项各扫一遍等于把一次遍历乘以选中项数）。
        let leaf_paths = vault
            .knowledge_metrics_facts()
            .map_err(vault_err)?
            .leaf_paths;
        let mut changes: Vec<(String, bool)> = Vec::new();
        for (from, to) in plan {
            let moves = remapped_leaf_paths(&leaf_paths, &from, &to);
            match vault.move_with_report(&from, &to) {
                Ok(report) => {
                    relinked.extend(report.rewritten.iter().cloned());
                    receipt.absorb(report);
                    metric_moves.extend(moves);
                    changes.push((from.clone(), true));
                    changes.push((to.clone(), false));
                    result.moved.push(MovedPathDto { from, to });
                }
                Err(error) => result.failed.push(VaultActionFailureDto {
                    path: from,
                    error: vault_err(error),
                }),
            }
        }
        (root, changes)
    };
    // 度量落库与广播都在**锁外**：它们是写 sqlite 与唤起派生刷新，把它们留在锁内
    // 等于让每一个想读库的人排在这次批量后面（`emit_vault_changes` 记的那次真机故障）。
    record_local_knowledge_metric_moves(&app, &root, metric_moves);
    emit_vault_changes(&app, changes);
    broadcast_relinked(&app, "main", &root, &relinked);
    result.receipt = receipt;
    Ok(result)
}

/// 一次移动的前后路径。
///
/// **返回它而不是让界面自己拼**：打开中的标签、选中项与展开集都要按新路径重映射，
/// 而"目标目录 + 末级名"这段拼接的权威版本在 Rust（本命令里那一段）。让前端为了重映射
/// 再拼一遍，等于把同一条规则养出第二份实现——两份一旦分叉，表现是移动完之后标签指向
/// 一个不存在的路径。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct MovedPathDto {
    pub from: String,
    pub to: String,
}

/// 批量动作里**没做成的那一项**：是哪一个、为什么。
///
/// **失败必须逐项带路径**：一句"移动失败"对一次四项的批量等于什么都没说——用户看到
/// 三个搬走了、一个还在原处，而界面既不说是哪一个，也不说为什么。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct VaultActionFailureDto {
    pub path: String,
    pub error: String,
}

/// 批量移动的结果：搬走了哪些（前后路径）、哪些没搬成（逐项理由）。
#[derive(Debug, Clone, Default, Serialize, Type)]
pub(crate) struct VaultMoveResultDto {
    pub moved: Vec<MovedPathDto>,
    pub failed: Vec<VaultActionFailureDto>,
    /// 搬成了的那几项一起的回执（链接处数、篇数、撤销凭据，PRD-06-030 ③）。
    pub receipt: RelocationReceiptDto,
}

/// 一次改名的结果：新路径与回执（PRD-06-030 ③）。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct VaultRenameDto {
    pub path: String,
    pub receipt: RelocationReceiptDto,
}

/// 批量删除的结果：删掉了哪些、哪些没删成（逐项理由）。
#[derive(Debug, Clone, Default, Serialize, Type)]
pub(crate) struct VaultDeleteResultDto {
    /// 真正不在了的那些（**含**动手前就已经不存在的——删除是幂等意图）。界面据它关标签、
    /// 清选中：报得多了会关掉一篇还在的笔记，报得少了会留下一个点不开的标签。
    pub deleted: Vec<String>,
    pub failed: Vec<VaultActionFailureDto>,
}

/// 这一批**没法开始**的原因（都是选择本身就能判定的，故整批拒绝）。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MovePlanRejection {
    /// 把一个文件夹拖进它自己（或它的子孙）里。
    IntoItself,
    /// 落点已被占用：磁盘上已有同名对象，或**这一批里另一项**要落到同一个名字上。
    TargetOccupied,
}

/// 一次批量移动的计划：`(from, to)` 的有序清单。
///
/// 纯函数（`exists` 由调用方注入），故四条规则可以无头测：剪枝子孙、跳过"已经在那儿"的、
/// 拒绝移进自身、拒绝落点占用（**含批内重名**）。
///
/// **批内重名必须在这里抓**：不同目录下的两篇同名笔记一起拖进同一个文件夹，磁盘预检
/// 全部通过，而第二项会在真正搬的时候撞上"目标已存在"——那正是"四个成功三个"的形状。
pub(crate) fn plan_moves(
    paths: &[String],
    target_dir: &str,
    exists: impl Fn(&str) -> bool,
) -> Result<Vec<(String, String)>, MovePlanRejection> {
    let mut plan: Vec<(String, String)> = Vec::new();
    for from in prune_selection(paths) {
        let to = moved_into_path(&from, target_dir);
        if to == from {
            // 已经在这个目录里了：不是错误，只是没有可做的事。
            continue;
        }
        if target_dir == from || target_dir.starts_with(&format!("{from}/")) {
            return Err(MovePlanRejection::IntoItself);
        }
        if exists(&to) || plan.iter().any(|(_, planned)| *planned == to) {
            return Err(MovePlanRejection::TargetOccupied);
        }
        plan.push((from, to));
    }
    Ok(plan)
}

/// 某个对象移动进 `target_dir` 后的库相对路径（`target_dir` 为空串 = 库根）。
fn moved_into_path(path: &str, target_dir: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    if target_dir.is_empty() {
        name.to_owned()
    } else {
        format!("{target_dir}/{name}")
    }
}

/// 一次移动/删除的**真实作用对象**：去重，并丢掉已被选中目录覆盖的子孙。
///
/// 多选里同时选中 `A/` 与 `A/b.md` 是常态（框选、Cmd 加选都会造出来）。不剪掉子孙的
/// 表现是：整个 `A/` 搬走之后，第二项落在一个已经不存在的旧路径上——一次合法的选择产出
/// 一条"文件不存在"的错误。归属由路径本身决定，故判据只有一条：前缀是否是另一个选中项。
pub(crate) fn prune_selection(paths: &[String]) -> Vec<String> {
    let mut unique: Vec<String> = Vec::new();
    for path in paths {
        let normalized = path.trim_end_matches('/');
        if !normalized.is_empty() && !unique.iter().any(|kept| kept == normalized) {
            unique.push(normalized.to_owned());
        }
    }
    unique
        .iter()
        .filter(|path| {
            !unique
                .iter()
                .any(|other| *other != **path && path.starts_with(&format!("{other}/")))
        })
        .cloned()
        .collect()
}

/// `from → to` 这一次移动带走的度量叶子路径（目录移动时按前缀整体重映射）。
fn remapped_leaf_paths(leaf_paths: &[String], from: &str, to: &str) -> Vec<(String, String)> {
    let prefix = format!("{from}/");
    leaf_paths
        .iter()
        .filter_map(|path| {
            if path == from {
                Some((path.clone(), to.to_owned()))
            } else {
                path.strip_prefix(&prefix)
                    .map(|suffix| (path.clone(), format!("{to}/{suffix}")))
            }
        })
        .collect()
}

/// 新建空文件夹（文件树「新建文件夹」，PRD-06-006）。
/// 列举某个归档目录下的全部子目录（PRD-07-062，2026-09-08）。
///
/// 录音 / 转写清单的文件夹树本来是从**已加载条目的路径**折出来的，于是没有条目的目录
/// 压根不生成节点——用户点「新建文件夹」，磁盘上确实建好了，清单里却永远看不见，而那颗
/// 按钮就摆在清单顶上（真机上报）。本命令把"这儿有哪些目录"作为磁盘事实读出来，前端据此
/// 把空目录补进树里；归属仍然只是路径，不引入第二事实源（PRD-07-062 的原则不变）。
///
/// # Errors
/// Vault 未打开时返回错误。目录不存在返回空表——归档目录可能还没被创建过，那不是错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_list_dirs(
    state: tauri::State<'_, AppState>,
    root_rel: String,
) -> Result<Vec<String>, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let dirs = vault.list_dirs(&root_rel).map_err(vault_err)?;
    Ok(without_attachment_dirs(&state.settings_snapshot(), dirs))
}

/// 从目录表里剔除**附件目录**（录音 / 导入媒体 / 图片附件的落点，默认 `assets`）。
///
/// 它们是笔记的附件容器，不是用户的分组（真机 2026-09-08：录音 / 转写清单里冒出
/// `assets`）。判据与落盘同源：一条目录是不是附件目录，看"与它同级的笔记按当前
/// `vault.attachment_location` / `vault.attachment_folder` 策略会把附件放到哪"——恰好就是
/// 它，就剔除。于是 `note_subfolder`（默认）下每一层的 `assets` 都被认出来，而
/// `vault_root` / `note_folder` 两种策略下附件与笔记同住、没有独立目录，什么都不剔。
/// 用户真起了一个叫 `assets` 的分组会被一并藏起——那正是附件会落进去的地方，两者本就分不开。
fn without_attachment_dirs(settings: &Settings, dirs: Vec<String>) -> Vec<String> {
    let is_attachment_dir = |dir: &str| {
        let probe = luna_vault::join_vault_rel(luna_vault::parent_dir(dir), "x.md");
        super::workspace::media_asset_dir(settings, &probe) == dir
    };
    dirs.into_iter()
        .filter(|dir| !is_attachment_dir(dir))
        .collect()
}

#[tauri::command]
#[specta::specta]
pub(crate) fn vault_create_dir(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    vault.create_dir(&path).map_err(vault_err)?;
    emit_vault_changed(&app, &path, false);
    Ok(())
}

/// 重命名**库内对象**——笔记、资料文件与文件夹同一条路（PRD-06-006 / PRD-07-059）。
///
/// 前端只提交用户看到的那个名字；同目录拼接、扩展名保留与非法字符拒绝全在 Rust
/// （ADR-003）。返回新的库相对路径。
///
/// **重命名是对象的能力，不是文件树的能力**：它此前叫 `rename_note`，只认 `.md`
/// 且恒补 `.md` 后缀——于是录音清单里根本没有这一项（改名被当成"文件树的行内输入态"），
/// 而在文件树里给一张 `assets/x.png` 改名会把它变成 `x.png.md`，媒体当场失联。
///
/// **扩展名不进参数面**：用户提交的名字不带原扩展名时由 Rust 原样补回。库里的音视频
/// 与附件靠扩展名被识别和播放，让改名顺手改掉类型只会制造"文件还在、但打不开了"。
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn vault_rename(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
    name: String,
) -> Result<VaultRenameDto, String> {
    let change_scope = VaultChangeScope::for_window(window.label());
    let i18n = state.copy();
    // 窗口感知（PRD-06-050）：副库窗口的重命名落它自己的库。是不是目录只有库答得了，
    // 故解析与落盘在同一次加锁里完成——中间放开锁，对象可能已经不是同一个类型了。
    let (target, root, report) = state.with_vault(window.label(), |vault| {
        let is_dir = vault.is_dir(&path).map_err(vault_err)?;
        let target = renamed_entry_path(i18n, &path, &name, is_dir)?;
        let root = vault.root().to_path_buf();
        if target == path {
            return Ok((target, root, None));
        }
        let report = vault.move_with_report(&path, &target).map_err(vault_err)?;
        Ok((target, root, Some(report)))
    })?;
    let Some(report) = report else {
        // 名字没变：不动文件、也不广播一次什么都没发生的变更。
        return Ok(VaultRenameDto {
            path: target,
            receipt: RelocationReceiptDto::default(),
        });
    };
    let relinked = report.rewritten.clone();
    let mut receipt = RelocationReceiptDto::default();
    receipt.absorb(report);
    if change_scope.affects_active_vault() {
        if let Some(root) = state.vault_root() {
            record_local_knowledge_metric_moves(&app, &root, [(path.clone(), target.clone())]);
        }
    }
    // 一次改名是**一个**动作：旧路径与新路径逐条广播（消费者按路径过滤），派生刷新只
    // 做一次（见 `emit_vault_changes`）。
    emit_vault_changes_in_scope(&app, [(path, true), (target.clone(), false)], change_scope);
    broadcast_relinked(&app, window.label(), &root, &relinked);
    Ok(VaultRenameDto {
        path: target,
        receipt,
    })
}

/// 重命名后的库相对路径：同目录、用户给的名字、**原扩展名**（目录没有扩展名）。
///
/// 用户已经把扩展名打进去的（`x.png` → `y.png`）不再重复追加——判据是"是否以原扩展名
/// 结尾"这一条确定的事实，不是猜他有没有打后缀。
pub(crate) fn renamed_entry_path(
    i18n: BackendCopy<'_>,
    current_path: &str,
    name: &str,
    is_dir: bool,
) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(coded_error_with(i18n, "INVALID_NAME", "invalid-name-empty"));
    }
    if trimmed == "." || trimmed == ".." {
        return Err(coded_error_with(
            i18n,
            "INVALID_NAME",
            "invalid-name-generic",
        ));
    }
    if trimmed
        .chars()
        .any(|ch| ch == '/' || ch == '\\' || ch == '\0' || ch.is_control())
    {
        return Err(coded_error_with(
            i18n,
            "INVALID_NAME",
            "invalid-name-separator",
        ));
    }

    let current = Path::new(current_path);
    let extension = if is_dir {
        None
    } else {
        current.extension().and_then(|ext| ext.to_str())
    };
    let file_name = match extension {
        Some(ext) if !trimmed.ends_with(&format!(".{ext}")) => format!("{trimmed}.{ext}"),
        _ => trimmed.to_owned(),
    };
    let target = current
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.join(&file_name))
        .unwrap_or_else(|| PathBuf::from(&file_name));
    // Vault 相对路径一律 `/`（Windows 上 `join` 拼出的是 `\`，前端拿它对不上标签页与文件树）。
    Ok(target.to_string_lossy().replace('\\', "/"))
}

/// 当前生效的删除策略（PRD-06-009 回收站三态 + 删除确认文案）。
///
/// **判定不下放到前端**：策略是 `vault.delete_mode` 经 `vault_delete_mode_from_settings`
/// 归一化的结果（`vault-trash` 连字符写法、缺省回落都在那一处），前端拿到的是一枚已经
/// 定好的值，只负责按它渲染（ADR-003）。回收站页据此在「库内清单 / 指向系统废纸篓 /
/// 永久删除警示」三态间选择，删除确认卡据此说出这一刀删到哪里去。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_delete_mode(state: tauri::State<'_, AppState>) -> DeleteModeDto {
    vault_delete_mode_from_settings(&state.settings_snapshot()).into()
}

/// 删除若干 Vault 对象；`mode=None` 时走 registry 默认策略（PRD-06-009）。
///
/// **N=1 不是特例**：多选删除若另起一条命令，"走系统回收站还是库内 `.trash`"这类
/// 策略迟早在两条路上各说一次。选择集先剪枝（去重 + 丢掉已被选中目录覆盖的子孙），
/// 再逐项删。
///
/// **已经不存在的对象按「已达成」计入 `deleted`**：删除表达的是"让它不在"，而多选删除
/// 与外部改动（另一个窗口、Finder）天然会撞上这一种。真实失败（权限、系统回收站不可用）
/// **只属于那一项**，逐项交代且不中止整批——已删掉的不回滚，它们在回收站里。
///
/// 广播在锁外、整批只触发一次派生刷新（`emit_vault_changes`）。
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn vault_delete(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
    mode: Option<DeleteModeDto>,
) -> Result<VaultDeleteResultDto, String> {
    let delete_mode = mode
        .map(Into::into)
        .unwrap_or_else(|| vault_delete_mode_from_settings(&state.settings_snapshot()));
    let targets = prune_selection(&paths);
    let mut result = VaultDeleteResultDto::default();
    let mut orphan_leaves: Vec<String> = Vec::new();
    let i18n = backend_copy_arc(&app);
    let (root, research_links) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        // 删之前记下哪些项目研究笔记的「素材」索引链着它们：删完一并去掉那一行（ADR-167 ③）。
        let research_links =
            super::creator_sources::research_links_to(vault, i18n.as_deref(), &targets);
        let leaf_paths = vault
            .knowledge_metrics_facts()
            .map(|facts| facts.leaf_paths)
            .unwrap_or_default();
        for path in targets {
            if !vault.exists(&path) {
                result.deleted.push(path);
                continue;
            }
            let target = Path::new(&path);
            match vault.delete(&path, delete_mode) {
                Ok(()) => {
                    orphan_leaves.extend(
                        leaf_paths
                            .iter()
                            .filter(|leaf| Path::new(leaf).starts_with(target))
                            .cloned(),
                    );
                    result.deleted.push(path);
                }
                Err(error) => result.failed.push(VaultActionFailureDto {
                    path,
                    error: vault_err(error),
                }),
            }
        }
        (vault.root().to_path_buf(), research_links)
    };
    record_local_knowledge_metric_deletions(&app, &root, orphan_leaves);
    for (research, note) in &research_links {
        if result.deleted.contains(note) {
            super::creator_quote::unlink_material(&app, research, note);
        }
    }
    emit_vault_changes(&app, result.deleted.iter().map(|path| (path.clone(), true)));
    Ok(result)
}

/// 列出一篇笔记的快照。
#[tauri::command]
#[specta::specta]
pub(crate) fn snapshots(state: tauri::State<'_, AppState>, path: String) -> Vec<SnapshotDto> {
    state
        .vault
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .as_ref()
                .map(|vault| vault.snapshots(&path).into_iter().map(Into::into).collect())
        })
        .unwrap_or_default()
}

/// 读取一篇笔记的指定快照内容（只读预览，不写回 Vault）。
#[tauri::command]
#[specta::specta]
pub(crate) fn read_snapshot(
    state: tauri::State<'_, AppState>,
    path: String,
    id: String,
) -> Result<String, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    vault.read_snapshot(&path, &id).map_err(vault_err)
}

/// 还原一篇笔记的指定快照。
#[tauri::command]
#[specta::specta]
pub(crate) fn restore_snapshot(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    id: String,
) -> Result<(), String> {
    let (root, current_fingerprint, target_fingerprint) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let current = vault.read(&path).map_err(vault_err)?.content;
        let target = vault.read_snapshot(&path, &id).map_err(vault_err)?;
        let facts = (
            vault.root().to_path_buf(),
            luna_vault::knowledge_body_hash(luna_vault::body_of(&current)),
            luna_vault::knowledge_body_hash(luna_vault::body_of(&target)),
        );
        vault.restore_snapshot(&path, &id).map_err(vault_err)?;
        facts
    };
    let _ = app
        .try_state::<crate::metrics::LocalKnowledgeMetrics>()
        .and_then(|metrics| {
            let grant = metrics.begin()?;
            metrics.with_store(grant, |store| {
                store.observe_restore(
                    &root.display().to_string(),
                    &path,
                    &current_fingerprint,
                    &target_fingerprint,
                    metric_now_ms(),
                )
            })
        });
    emit_vault_changed(&app, &path, false);
    Ok(())
}

/// 列出 Vault 内 `.trash/` 回收站，**顺带按保留期裁掉过期的那些**。
///
/// 裁剪挂在这条读命令上是刻意的：没有后台定时器，而一台几个月没开的设备，回收站该在
/// 用户下次打开它的那一刻就已经是干净的——而不是等某个循环转到，或者让他看着一屏早该
/// 消失的条目。过期为空时它是一次纯读（几个 stat），没有代价。
///
/// 每条顺带算出「还有几天自动删除」（保留期设为「永不」时为 `None`）。
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn list_trash(state: tauri::State<'_, AppState>) -> Vec<TrashEntryDto> {
    let max_age_ms = trash_retention_ms(&state.settings_snapshot());
    let now = now_ms_for_registry();
    state
        .vault
        .lock()
        .ok()
        .and_then(|guard| {
            guard.as_ref().map(|vault| {
                // 裁剪失败（权限 / 只读盘）不该让"看一眼回收站"整个失败——那些条目留在
                // 列表里可见、可手动重试，与 `purge` 的失败口径一致。
                let _ = vault.prune_trash(max_age_ms, now);
                vault
                    .list_trash()
                    .into_iter()
                    .map(|entry| TrashEntryDto::from_entry(entry, max_age_ms, now))
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// 预览 Vault 内 `.trash/` 条目内容（永久删除前判断，PRD-06-009）。
#[tauri::command]
#[specta::specta]
pub(crate) fn read_trash_content(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<TrashPreviewDto, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    vault.trash_preview(&id).map(Into::into).map_err(vault_err)
}

/// 还原落点被占时的处置 DTO（[`RestoreConflict`] 的线上形态）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum RestoreConflictDto {
    /// 报错，什么都不动（缺省；界面据此去问用户要不要留副本）。
    Fail,
    /// 还原为副本（落点取唯一名），原地那个文件不受影响。
    KeepBoth,
}

impl From<RestoreConflictDto> for RestoreConflict {
    fn from(value: RestoreConflictDto) -> Self {
        match value {
            RestoreConflictDto::Fail => Self::Fail,
            RestoreConflictDto::KeepBoth => Self::KeepBoth,
        }
    }
}

/// 永久删除的结果 DTO（成了几件 / 哪几件没成）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct PurgeResultDto {
    /// 真正抹掉的条目数（含随主条目一起走的附件）。
    pub purged: u32,
    /// 未能抹掉的条目原路径；它们仍留在回收站里，界面上照旧可见、可重试。
    pub failed: Vec<String>,
}

impl From<PurgeOutcome> for PurgeResultDto {
    fn from(value: PurgeOutcome) -> Self {
        Self {
            purged: value.purged,
            failed: value.failed,
        }
    }
}

/// 从 Vault 内 `.trash/` 还原条目；返回**实际**落点（副本形态下与原路径不同）。
///
/// `conflict=None` 等同 `fail`：默认不擅自改名，先让界面把「原位置已被占」这件事说给
/// 用户听，再由他决定要不要留副本。
#[tauri::command]
#[specta::specta]
pub(crate) fn restore(
    state: tauri::State<'_, AppState>,
    id: String,
    conflict: Option<RestoreConflictDto>,
) -> Result<String, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    vault
        .restore(&id, conflict.map_or(RestoreConflict::Fail, Into::into))
        .map_err(vault_err)
}

/// 清空或永久删除 Vault 内 `.trash/` 条目。
#[tauri::command]
#[specta::specta]
pub(crate) fn purge(
    state: tauri::State<'_, AppState>,
    id: Option<String>,
) -> Result<PurgeResultDto, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    vault
        .purge(id.as_deref())
        .map(Into::into)
        .map_err(vault_err)
}

pub(crate) fn ai_artifact_rel_path(settings: &Settings, file_stem: &str) -> String {
    let dir = safe_vault_relative_dir(settings, "vault.ai_artifacts_dir", "AI");
    format!("{dir}/{file_stem}.md")
}

pub(crate) fn translate_space_note_body(source: &str, translation: &str) -> String {
    format!("## 原文\n\n{source}\n\n## 翻译\n\n{translation}\n")
}

/// 将单次 AI 结果存入 Vault 普通笔记（落 `vault.ai_artifacts_dir`）。路径、类型与模板由
/// Rust 决定，前端只传结果正文。
///
/// **`type: note` 而非 `quick-note`**（PRD-02-055 分家裁决，2026-07-28）：知识分层
/// （ADR-024）建立后这个归属变得显然——用户**显式保存**的 AI 结果与蒸馏产物同理
/// （蒸馏产物即 `type: note`），是知识资产候选而不是语音速记；此前共用 `quick-note`
/// 让速记清单混进 `result-<JobId>.md`，收件箱语义立不住。
#[tauri::command]
#[specta::specta]
pub(crate) fn save_ai_result_to_note(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    body: String,
) -> Result<NoteDocDto, String> {
    if body.trim().is_empty() {
        return Err(coded_error(&app, "EMPTY_RESULT", "empty-result-note"));
    }
    let settings = state.settings_snapshot();
    let rel = ai_artifact_rel_path(&settings, &format!("result-{}", luna_core::JobId::new()));
    let intent = knowledge::prepare_knowledge_write(&app, &rel, &body, "chat_writeback", false)?;
    let write_result = (|| -> Result<NoteDoc, String> {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault
            .create(
                &rel,
                NoteType::Note,
                &luna_vault::TemplateVars {
                    body: body.clone(),
                    ..luna_vault::TemplateVars::default()
                },
            )
            .map_err(vault_err)
    })();
    let doc = match write_result {
        Ok(doc) => doc,
        Err(error) => {
            knowledge::abandon_knowledge_write(&app, intent);
            return Err(error);
        }
    };
    emit_vault_changed(&app, &doc.path, false);
    knowledge::commit_knowledge_write(&app, intent)?;
    Ok(doc.into())
}

/// 将译境捕捉的原文与译文保存为普通笔记；保存目录只由 Vault 设置决定，前端不拼路径。
#[tauri::command]
#[specta::specta]
pub(crate) fn save_translate_space_result_to_note(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    translation: String,
) -> Result<NoteDocDto, String> {
    if translation.trim().is_empty() {
        return Err(coded_error(&app, "EMPTY_RESULT", "empty-result-lens-note"));
    }
    let source = {
        let session = state.popup.lock().map_err(|_| "会话锁中毒".to_string())?;
        if !session.save_to_translate_space() {
            return Err("当前翻译结果并非来自译境捕捉。".to_owned());
        }
        session
            .source_text()
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| "译境原文不可用，无法存入笔记。".to_owned())?
    };
    let settings = state.settings_snapshot();
    let dir = safe_vault_relative_dir(&settings, "vault.translate_space_dir", "TranslateSpace");
    let rel = format!("{dir}/translate-space-{}.md", luna_core::JobId::new());
    let body = translate_space_note_body(&source, &translation);
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let doc = create_unique_note(state.copy(), vault, &rel, NoteType::Note, body)?;
    emit_vault_changed(&app, &doc.path, false);
    Ok(doc.into())
}

/// 返回受管本地音频 Range 服务信息。
#[tauri::command]
#[specta::specta]
pub(crate) fn media_server_info(state: tauri::State<'_, AppState>) -> MediaServerInfoDto {
    MediaServerInfoDto {
        base_url: state.media_server.base_url().to_string(),
        token: state.media_server.token().to_string(),
    }
}

/// 一条**标准 Markdown 链接**（`[标题](相对路径)`）指向的库内笔记；不是笔记就是 `None`。
///
/// 全部自动生成的溯源链接都是这个形态（PRD-06-030：自动生成的链接恒为标准相对 md 链接），
/// 蒸馏 / 汇总 / 主题页初稿的「来源」区逐条如此。而阅读视图此前只认 `[[wiki]]` 与 `#tag`，
/// 于是那一整区在屏幕上是**纯文本**——用户点不动自己刚生成的那篇产物的来源（真机
/// 2026-09-09）。
///
/// **解析在 Rust**（ADR-003）：`../` 的归一、越界判定与"它到底存不存在"是文件系统的
/// 事实，前端拼一遍就是第二处会漂的定义，而漂了之后的表现是把用户送到一篇不存在的笔记。
/// 复用 `resolve_note_relative_asset`（图片走的同一条），只是把结论换成**库内相对路径**。
///
/// 百分号编码由本层解开：链接是 Rust 自己写出去的（`encode_md_link`），解也归它。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_link_note_target(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    note_path: String,
    href: String,
) -> Result<Option<String>, String> {
    // 带 scheme 的（http/mailto/…）不是库内链接，交给外部打开那条路。
    if href.contains("://") || href.starts_with("mailto:") || href.starts_with('#') {
        return Ok(None);
    }
    // 片段（`#L12` / `#t=83456`，ADR-162 ⑭）只决定落到哪里，不影响指向哪一篇。
    let href = href.split_once('#').map_or(href.as_str(), |(path, _)| path);
    let decoded = percent_decode(href);
    if !decoded.to_ascii_lowercase().ends_with(".md") {
        // 只回答"指向哪一篇笔记"。附件、目录与外链各有各的路，这里不替它们决定。
        return Ok(None);
    }
    // 按窗口身份取库（PRD-06-050）：副库窗口里点的链接指向副库里的笔记。
    let Ok(root) = state.with_vault(window.label(), |vault| Ok(vault.root().to_path_buf())) else {
        return Ok(None);
    };
    let Ok(absolute) =
        crate::media_server::resolve_note_relative_asset(&root, &note_path, &decoded)
    else {
        // 指不到 / 越界 / 不存在：如实说没有，不猜一个"大概是这篇"。
        return Ok(None);
    };
    let Ok(canonical_root) = root.canonicalize() else {
        return Ok(None);
    };
    Ok(absolute
        .strip_prefix(&canonical_root)
        .ok()
        .map(|rel| rel.to_string_lossy().replace('\\', "/")))
}

/// 点击一条标准 md 链接要去的地方（ADR-162 ⑭）：哪一篇、到哪一行、从哪一刻播放。
#[derive(Debug, Clone, Serialize, Type)]
pub struct NoteLinkLocationDto {
    pub path: String,
    /// 到哪一行（1 起，含 frontmatter）；`None` = 只打开。
    pub line: Option<u32>,
    /// 从哪一刻播放（毫秒）；`None` = 不播放。
    pub seek_ms: Option<f64>,
}

/// 点击一条标准 md 链接：先按 [`vault_link_note_target`] 认出指向哪一篇，再把片段 `#L<行>` /
/// `#t=<毫秒>` 解析成行与时刻（时刻落在哪一行看那篇的时间码行）。编辑面与阅读面同一条命令。
///
/// # Errors
/// 不报错：指不到为 `Ok(None)`。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn vault_link_location(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    note_path: String,
    href: String,
) -> Result<Option<NoteLinkLocationDto>, String> {
    let fragment = href.split_once('#').map(|(_, f)| f.to_owned());
    let Some(path) = vault_link_note_target(window.clone(), state.clone(), note_path, href)? else {
        return Ok(None);
    };
    let location = match fragment {
        Some(fragment) => state
            .with_vault(window.label(), |vault| {
                vault.read(&path).map(|doc| doc.content).map_err(vault_err)
            })
            .map(|content| luna_vault::quote::link_location(&content, &fragment))
            .unwrap_or(luna_vault::quote::LinkLocation {
                line: None,
                seek_ms: None,
            }),
        None => luna_vault::quote::LinkLocation {
            line: None,
            seek_ms: None,
        },
    };
    Ok(Some(NoteLinkLocationDto {
        path,
        line: location.line,
        #[allow(clippy::cast_precision_loss)]
        seek_ms: location.seek_ms.map(|ms| ms as f64),
    }))
}

/// 一批 wikilink 目标各自指向哪一篇（编辑器 / 阅读视图着色用）；指不到为 `None`。
///
/// 解析与图谱、反链同一份 `LinkIndex`（ADR-003）：同名笔记先取写链接那篇所在目录的，
/// 再取层级最浅的。窗口感知：副库窗口解析它自己的库（PRD-06-050）。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_resolve_wikilinks(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    note_path: String,
    targets: Vec<String>,
) -> Result<Vec<Option<String>>, String> {
    state.with_vault(window.label(), |vault| {
        vault
            .resolve_wikilinks(&note_path, &targets)
            .map_err(vault_err)
    })
}

/// 点击一条 wikilink：指向哪一篇、落到第几行 / 第几个标题；指不到时给出该新建的路径。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_resolve_wikilink(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    note_path: String,
    target: String,
) -> Result<WikiLinkHitDto, String> {
    state.with_vault(window.label(), |vault| {
        vault
            .resolve_wikilink(&note_path, &target)
            .map(Into::into)
            .map_err(vault_err)
    })
}

/// 一条嵌入 `![[…]]` 指向什么（对标方案 D5）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum EmbedHitDto {
    /// 库里的文件；`link` 是从宿主笔记指向它的相对路径（未转义），按 `![](link)` 渲染。
    File {
        path: String,
        link: String,
        image: bool,
    },
    /// 一篇笔记或其中一节：正文已去 frontmatter，相对链接已按宿主改写。
    Note {
        path: String,
        content: String,
    },
    Missing,
}

impl From<luna_vault::embed::EmbedHit> for EmbedHitDto {
    fn from(hit: luna_vault::embed::EmbedHit) -> Self {
        use luna_vault::embed::EmbedHit as H;
        match hit {
            H::File { path, link, image } => Self::File { path, link, image },
            H::Note { path, content } => Self::Note { path, content },
            H::Missing => Self::Missing,
        }
    }
}

/// 一批嵌入目标（`|` 之前的文字）各自指向什么；解析与 wikilink、别名同一份（ADR-003）。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_resolve_embeds(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    note_path: String,
    targets: Vec<String>,
) -> Result<Vec<EmbedHitDto>, String> {
    state.with_vault(window.label(), |vault| {
        vault
            .resolve_embeds(&note_path, &targets)
            .map(|hits| hits.into_iter().map(Into::into).collect())
            .map_err(vault_err)
    })
}

/// 从文件树把条目拖进正文（对标方案 B4）：每个条目一条**标准相对 md 链接**（PRD-06-030，
/// 与蒸馏来源区等自动生成的链接同一构造点 `markdown_link`）。笔记的链接文字是文件名去掉
/// `.md`，其它文件是文件名；文件夹跳过。顺序同拖入。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_link_markdown(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    note_path: String,
    targets: Vec<String>,
) -> Result<Vec<String>, String> {
    state.with_vault(window.label(), |vault| {
        Ok(targets
            .iter()
            .filter(|target| vault.root().join(target.as_str()).is_file())
            .map(|target| {
                let name = target.rsplit('/').next().unwrap_or(target);
                let text = name.strip_suffix(".md").unwrap_or(name);
                luna_vault::markdown_link(&note_path, target, text)
            })
            .collect())
    })
}

/// `[[` 补全候选：标题、要写进链接的文字（同名笔记写路径）与所在目录。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_wikilink_candidates(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    note_path: String,
    query: String,
) -> Result<Vec<WikiLinkCandidateDto>, String> {
    state.with_vault(window.label(), |vault| {
        vault
            .wikilink_candidates(&note_path, &query, WIKILINK_CANDIDATE_LIMIT)
            .map(|items| items.into_iter().map(Into::into).collect())
            .map_err(vault_err)
    })
}

/// 补全列表的长度上限：再多就该继续打字缩小范围了。
const WIKILINK_CANDIDATE_LIMIT: usize = 50;

/// `%E4%B8%AD` → `中`。非法序列原样保留——链接文本里出现的孤立 `%` 不该让整条解析失败。
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 将 Markdown 图片相对路径解析为已锁定在 Vault 根内的绝对路径；前端再转 `asset:` URL。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_asset_path(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    note_path: String,
    asset_path: String,
) -> Result<String, String> {
    // 按窗口身份取库（PRD-06-050）：副库笔记里的图片在副库里找。
    let root = state.with_vault(window.label(), |vault| Ok(vault.root().to_path_buf()))?;
    resolve_note_relative_asset(&root, &note_path, &asset_path)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|e| format!("asset path rejected: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(list: &[&str]) -> Vec<String> {
        list.iter().map(|d| (*d).to_owned()).collect()
    }

    /// 百分号解码：链接是 Rust 自己写出去的（`encode_md_link`），解也归它。
    #[test]
    fn percent_decode_handles_generated_links_and_leaves_junk_alone() {
        assert_eq!(percent_decode("cutover-%E8%BF%81%E7%A7%BB"), "cutover-迁移");
        assert_eq!(percent_decode("a%20b.md"), "a b.md");
        // 孤立的 `%` 不该让整条解析失败——原样保留，后面那一步自然指不到就是了。
        assert_eq!(percent_decode("100%.md"), "100%.md");
        assert_eq!(percent_decode("%zz.md"), "%zz.md");
    }

    #[test]
    fn attachment_dirs_are_hidden_from_the_folder_tree_at_every_depth() {
        // 默认策略 note_subfolder / assets：每一层的 `assets` 都是附件目录。
        let settings = Settings::new();
        let out = without_attachment_dirs(
            &settings,
            dirs(&[
                "Recordings/abcde",
                "Recordings/abcde/assets",
                "Recordings/assets",
                "Recordings/资料",
            ]),
        );
        assert_eq!(out, dirs(&["Recordings/abcde", "Recordings/资料"]));
    }

    #[test]
    fn attachment_dir_name_follows_the_setting_not_a_hardcoded_assets() {
        let mut settings = Settings::new();
        settings.set("vault.attachment_folder", "附件");
        let out =
            without_attachment_dirs(&settings, dirs(&["Recordings/assets", "Recordings/附件"]));
        // 改了附件目录名之后 `assets` 只是一个普通分组，`附件` 才是要藏的那个。
        assert_eq!(out, dirs(&["Recordings/assets"]));
    }

    #[test]
    fn strategies_without_a_separate_attachment_dir_hide_nothing() {
        let mut settings = Settings::new();
        settings.set("vault.attachment_location", "note_folder");
        let out = without_attachment_dirs(&settings, dirs(&["Recordings/assets", "Recordings/A"]));
        assert_eq!(out, dirs(&["Recordings/assets", "Recordings/A"]));
    }
}
