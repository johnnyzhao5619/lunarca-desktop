//! 冷启动时间线（TDD-13 §6「冷启动到可响应热键 < 1.5s」的度量落点，ADR-114 ①）。
//!
//! 职责：从进程入口起记一串**里程碑**（setup 开始/结束、热键注册完成、首帧、工作区就绪、
//! 引擎就绪），以毫秒偏移落到设备级诊断文件；设置 › 系统与诊断读回「上一次启动」。
//!
//! 只记时刻，**不记任何用户内容**（TDD-12 脱敏纪律），默认离线、不上报。
//! 前端只发「首帧」「工作区就绪」两枚标记，判定与持久化都在这里（ADR-003）。

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use specta::Type;

/// 诊断文件名（app_data_dir 下）。
pub(crate) const DIAGNOSTICS_FILE: &str = "startup-diagnostics.json";

/// 里程碑名——**封闭集合**，前端只能打这两枚；其余由 Rust 自己打。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum StartupMilestone {
    /// Tauri `setup` 开始。
    SetupBegin,
    /// Tauri `setup` 结束（窗口、面板、托盘、热键全部装配完）。
    SetupEnd,
    /// 全局热键注册完成 = 「可响应热键」（TDD-13 §6 的那条指标）。
    ShortcutsRegistered,
    /// 主窗首帧（前端挂载并读回引导状态后打）。
    FirstFrame,
    /// 工作区就绪（库树与清单首轮取回）。
    WorkspaceReady,
    /// 本机推理引擎首轮编排完成（随包 llama-server，ADR-143）。
    EngineReady,
}

impl StartupMilestone {
    /// 前端可打的里程碑。其余两枚由 Rust 内部打，前端伪造不了。
    #[must_use]
    pub const fn frontend_allowed(self) -> bool {
        matches!(self, Self::FirstFrame | Self::WorkspaceReady)
    }
}

/// 一枚里程碑：名字 + 距进程起点的毫秒数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct StartupMarkDto {
    pub milestone: StartupMilestone,
    pub elapsed_ms: u32,
}

/// 「上一次启动 / 本次启动」两份时间线。
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct StartupDiagnosticsDto {
    /// 本次启动到目前为止已到达的里程碑。
    pub current: Vec<StartupMarkDto>,
    /// 上一次启动的完整时间线；首次运行为 `None`。
    pub previous: Option<Vec<StartupMarkDto>>,
}

/// 进程级时间线（`app.manage`）。
pub struct StartupTimeline {
    origin: Instant,
    marks: Mutex<Vec<StartupMarkDto>>,
    /// 上一次启动的读回（启动时读一次，之后只读）。
    previous: Mutex<Option<Vec<StartupMarkDto>>>,
    /// 诊断文件位置；`None` = 无可写目录（测试 / 沙箱），只在内存里记。
    file: Option<PathBuf>,
}

impl StartupTimeline {
    /// 以「现在」为起点建立时间线。`app_data_dir` 缺失时不落盘。
    #[must_use]
    pub fn new(origin: Instant, app_data_dir: Option<PathBuf>) -> Self {
        let file = app_data_dir.map(|dir| dir.join(DIAGNOSTICS_FILE));
        let previous = file
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<Vec<StartupMarkDto>>(&bytes).ok());
        Self {
            origin,
            marks: Mutex::new(Vec::new()),
            previous: Mutex::new(previous),
            file,
        }
    }

    /// 记一枚里程碑（幂等：同一枚只记第一次；返回 `true` = 这次是首次记录）。
    /// 到达「工作区就绪」即落盘。
    pub fn mark(&self, milestone: StartupMilestone) -> bool {
        let elapsed_ms = u32::try_from(self.origin.elapsed().as_millis()).unwrap_or(u32::MAX);
        let snapshot = {
            let Ok(mut marks) = self.marks.lock() else {
                return false;
            };
            if marks.iter().any(|mark| mark.milestone == milestone) {
                return false;
            }
            marks.push(StartupMarkDto {
                milestone,
                elapsed_ms,
            });
            marks.clone()
        };
        if milestone == StartupMilestone::WorkspaceReady {
            self.persist(&snapshot);
        }
        true
    }

    fn persist(&self, marks: &[StartupMarkDto]) {
        let Some(path) = &self.file else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(marks) {
            if let Err(error) = std::fs::write(path, bytes) {
                eprintln!("[startup] 写启动诊断失败：{error}");
            }
        }
    }

    /// 读回两份时间线。
    #[must_use]
    pub fn snapshot(&self) -> StartupDiagnosticsDto {
        StartupDiagnosticsDto {
            current: self.marks.lock().map(|m| m.clone()).unwrap_or_default(),
            previous: self.previous.lock().ok().and_then(|p| p.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_are_idempotent_and_monotonic() {
        let timeline = StartupTimeline::new(Instant::now(), None);
        assert!(timeline.mark(StartupMilestone::SetupBegin));
        assert!(!timeline.mark(StartupMilestone::SetupBegin));
        assert!(timeline.mark(StartupMilestone::SetupEnd));
        let snapshot = timeline.snapshot();
        assert_eq!(snapshot.current.len(), 2);
        assert_eq!(snapshot.current[0].milestone, StartupMilestone::SetupBegin);
        assert!(snapshot.current[0].elapsed_ms <= snapshot.current[1].elapsed_ms);
        assert!(snapshot.previous.is_none());
    }

    #[test]
    fn workspace_ready_persists_and_next_run_reads_it_back() {
        let dir = tempfile::tempdir().unwrap();
        let first = StartupTimeline::new(Instant::now(), Some(dir.path().to_path_buf()));
        first.mark(StartupMilestone::SetupBegin);
        first.mark(StartupMilestone::WorkspaceReady);
        let second = StartupTimeline::new(Instant::now(), Some(dir.path().to_path_buf()));
        let previous = second.snapshot().previous.expect("previous run persisted");
        assert_eq!(previous.len(), 2);
        assert_eq!(previous[1].milestone, StartupMilestone::WorkspaceReady);
    }

    /// 前端只能打首帧与工作区就绪；其余由 Rust 打，前端伪造不了「热键已就绪」。
    #[test]
    fn only_two_milestones_are_frontend_facing() {
        assert!(StartupMilestone::FirstFrame.frontend_allowed());
        assert!(StartupMilestone::WorkspaceReady.frontend_allowed());
        assert!(!StartupMilestone::ShortcutsRegistered.frontend_allowed());
        assert!(!StartupMilestone::SetupEnd.frontend_allowed());
    }
}
