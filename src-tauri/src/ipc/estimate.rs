//! 任务前预估（PRD-13-040 ②，纵切 3 C4，IPC 层）。
//!
//! 长音频转写、批量蒸馏这类一次要花掉一大截额度的任务，在**开始之前**告诉用户「约消耗 X 分钟 /
//! 约 Y credits」；额度或自设上限不够时当场说清，而不是跑到一半失败。
//!
//! 三条纪律：
//! - **要不要问由 Rust 判**：只有这件任务此刻真会交给服务端时才问（转写看批量档那条腿
//!   [`AppState::remote_batch_asr`]，蒸馏看摘要能力的落点 `llm_capability_on_server`）；本机跑、
//!   未登录、严格本地一律不问、一个字节都不发。
//! - **问不到绝不挡路**：量算不出、服务端不答、超时，一律返回 `None`，界面照常开始——预估是善意
//!   提醒，服务端预留才是真正的闸。
//! - **结果是「约」**：最终账单以服务端计量为准；换算（分钟、credits）在 `luna_cloud::usage` 做完。

use super::*;

use luna_cloud::usage::{EstimateQuantity, UsageEstimateDto};

/// 一件待开始的任务（界面在「开始」之前把它原样交来）。
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum PendingTaskDto {
    /// 对一篇已有音频的笔记做二次转写（PRD-04-030）。
    Retranscribe { note_rel: String },
    /// 导入这些音 / 视频文件做批量转写（PRD-04-032 / PRD-05-001：选择框选好的或拖入的）。
    ImportMedia { paths: Vec<String> },
    /// 整理一个范围里待整理的全部资料（批量确认卡，PRD-06-044）。
    DistillScope {
        scope: Option<String>,
        full_text: Option<bool>,
    },
    /// 整理这几篇（右键单篇 / 多选 / 徽标，PRD-06-054）。
    DistillNotes {
        note_rels: Vec<String>,
        full_text: Option<bool>,
    },
}

/// 问一次预估最多等多久。**超时即当「不知道」，照常开始**（理由同蒸馏余量预检那一趟：
/// 控制面客户端没有默认请求超时，挂住的网络不能让「开始」按钮按下去没反应）。
const ESTIMATE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// 服务端计量用的能力名（与 `luna_core::Capability::name` 同一取值空间）。
const TRANSCRIBE_CAPABILITY: &str = "transcribe";
/// 蒸馏走 `JobKind::Summarize`（distill 模块头注）。
const DISTILL_CAPABILITY: &str = "summarize";

/// 一件转写任务该问的量：各段音频秒数之和。**任何一段时长不知道就不问**——少算一段的预估比
/// 没有预估更糟（「约 3 分钟」而实际是一小时）；零时长同样没什么可问。
fn asr_quantity(on_server: bool, durations: &[Option<f64>]) -> Option<EstimateQuantity> {
    if !on_server || durations.is_empty() {
        return None;
    }
    let mut total = 0.0f64;
    for secs in durations {
        let secs = (*secs).filter(|s| s.is_finite() && *s > 0.0)?;
        total += secs;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let seconds = total.ceil() as u64;
    (seconds > 0).then_some(EstimateQuantity::AsrSeconds(seconds))
}

/// 一件蒸馏任务该问的量（`need` = 估出的（输入，输出）token；`None` = 一篇都估不出）。
fn distill_quantity(on_server: bool, need: Option<(u64, u64)>) -> Option<EstimateQuantity> {
    if !on_server {
        return None;
    }
    let (input, output) = need?;
    (input + output > 0).then_some(EstimateQuantity::Tokens { input, output })
}

/// 批量档转写此刻会不会交给服务端：已登录且不在严格本地（门控），且批量档那条腿就绪。
fn batch_asr_on_server(app: &tauri::AppHandle, state: &AppState) -> bool {
    crate::cloud::evaluate_for(app).allowed()
        && matches!(
            state.remote_batch_asr(&state.settings_snapshot()),
            RemoteBatchAsr::Ready(_)
        )
}

/// 一篇笔记的二次转写各段音频的时长：媒体表里记着就用，没记就只读文件头（不解码）。
fn retranscribe_durations(state: &AppState, note_rel: &str) -> Option<Vec<Option<f64>>> {
    let parts: Vec<(Option<f64>, PathBuf)> = {
        let guard = state.vault.lock().ok()?;
        let vault = guard.as_ref()?;
        let doc = vault.read(note_rel).ok()?;
        sorted_media_parts(&doc.meta.media)
            .into_iter()
            .map(|m| (m.duration, vault.root().join(&m.path)))
            .filter(|(_, abs)| abs.is_file())
            .collect()
    };
    Some(
        parts
            .into_iter()
            .map(|(known, abs)| known.or_else(|| luna_audio::probe_media_duration_secs(&abs)))
            .collect(),
    )
}

/// 要导入的文件里那些音 / 视频的时长（筛选与 `import_media_paths` 同一判据：是文件、是认得的媒体）。
fn import_durations(paths: &[String]) -> Vec<Option<f64>> {
    paths
        .iter()
        .map(PathBuf::from)
        .filter(|p| p.is_file() && media_kind(&p.to_string_lossy()).is_some())
        .map(|p| luna_audio::probe_media_duration_secs(&p))
        .collect()
}

/// 把一件任务变成「问什么」：能力名 + 量。`None` = 这件任务不需要预估（本机跑、量算不出）。
async fn plan(
    app: &tauri::AppHandle,
    task: PendingTaskDto,
) -> Option<(&'static str, EstimateQuantity)> {
    let state = app.state::<AppState>();
    match task {
        PendingTaskDto::Retranscribe { note_rel } => {
            if !batch_asr_on_server(app, &state) {
                return None;
            }
            let app = app.clone();
            let durations = tauri::async_runtime::spawn_blocking(move || {
                retranscribe_durations(&app.state::<AppState>(), &note_rel)
            })
            .await
            .ok()??;
            asr_quantity(true, &durations).map(|q| (TRANSCRIBE_CAPABILITY, q))
        }
        PendingTaskDto::ImportMedia { paths } => {
            if !batch_asr_on_server(app, &state) {
                return None;
            }
            let durations = tauri::async_runtime::spawn_blocking(move || import_durations(&paths))
                .await
                .ok()?;
            asr_quantity(true, &durations).map(|q| (TRANSCRIBE_CAPABILITY, q))
        }
        PendingTaskDto::DistillScope { scope, full_text } => {
            if !llm_capability_on_server(app, &state, DISTILL_CAPABILITY) {
                return None;
            }
            let targets = distill_scope_targets(&state, scope.as_deref())?;
            distill_quantity(true, distill_token_need(app, &targets, full_text))
                .map(|q| (DISTILL_CAPABILITY, q))
        }
        PendingTaskDto::DistillNotes {
            note_rels,
            full_text,
        } => {
            if !llm_capability_on_server(app, &state, DISTILL_CAPABILITY) {
                return None;
            }
            distill_quantity(true, distill_token_need(app, &note_rels, full_text))
                .map(|q| (DISTILL_CAPABILITY, q))
        }
    }
}

/// 一件任务开始之前的用量预估（PRD-13-040 ②）。`None` = 不需要 / 问不到——界面照常开始，不说话。
///
/// 只在这件任务此刻真会交给服务端时才发请求（未登录、严格本地、本机跑都在发请求前返回）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn task_estimate(
    app: tauri::AppHandle,
    task: PendingTaskDto,
) -> Option<UsageEstimateDto> {
    let (capability, quantity) = plan(&app, task).await?;
    let client = app
        .try_state::<luna_cloud::commands::CloudState>()
        .map(|state| Arc::clone(&state.0))?;
    tokio::time::timeout(
        ESTIMATE_TIMEOUT,
        client.usage_estimate(capability, quantity),
    )
    .await
    .ok()?
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 转写：只在服务端跑时问；任何一段时长不知道、或总长为零都不问；秒数向上取整。
    #[test]
    fn asr_estimates_only_on_the_server_and_only_with_every_duration_known() {
        assert_eq!(
            asr_quantity(true, &[Some(60.0), Some(29.2)]),
            Some(EstimateQuantity::AsrSeconds(90))
        );
        assert_eq!(asr_quantity(false, &[Some(60.0)]), None, "本机跑不问");
        assert_eq!(
            asr_quantity(true, &[Some(60.0), None]),
            None,
            "少算一段的预估比没有预估更糟"
        );
        assert_eq!(asr_quantity(true, &[]), None);
        assert_eq!(asr_quantity(true, &[Some(0.0)]), None);
        assert_eq!(asr_quantity(true, &[Some(f64::NAN)]), None);
    }

    /// 蒸馏：只在服务端跑时问；估不出（一篇都没读到）或为零都不问。
    #[test]
    fn distill_estimates_only_on_the_server_with_a_known_need() {
        assert_eq!(
            distill_quantity(true, Some((1200, 800))),
            Some(EstimateQuantity::Tokens {
                input: 1200,
                output: 800
            })
        );
        assert_eq!(distill_quantity(false, Some((1200, 800))), None);
        assert_eq!(distill_quantity(true, None), None);
        assert_eq!(distill_quantity(true, Some((0, 0))), None);
    }

    /// 拖入的东西里只有认得的媒体才算（与入队同一判据）；不是文件的、认不出的都不进时长表。
    #[test]
    fn import_durations_skip_what_would_not_be_queued() {
        let dir = std::env::temp_dir().join(format!("luna-estimate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("notes.txt");
        std::fs::write(&doc, b"not media").unwrap();
        let bogus = dir.join("broken.wav");
        std::fs::write(&bogus, b"not really audio").unwrap();
        let durations = import_durations(&[
            doc.to_string_lossy().into_owned(),
            bogus.to_string_lossy().into_owned(),
            dir.join("missing.mp3").to_string_lossy().into_owned(),
        ]);
        // 只有那个 `.wav` 会被入队；它的头读不出时长 → None → 整件不给预估。
        assert_eq!(durations, [None]);
        assert_eq!(asr_quantity(true, &durations), None);
    }
}
