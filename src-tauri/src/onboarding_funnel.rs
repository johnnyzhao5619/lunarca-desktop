//! 首启引导的**本机**漏斗记录（PRD-14 §1「激活率命门」的度量落点）。
//!
//! 记的只有两样：什么时刻进了哪一步、最终有没有收场。据此能回答「停在哪一步」「每步花了
//! 多久」「这台设备上引导走完没有」——而在此之前这三个问题只能靠回忆。真机作业时尤其明显：
//! 同一轮验收里要判断「是我自己停在那一屏，还是它卡住了」，没有记录就只能猜。
//!
//! **它不是聚合漏斗。** 文件落在设备级目录、默认离线、不上报（与
//! [`crate::startup::StartupTimeline`] 同一条纪律，ADR-114 ①）。要拿到"一群用户在哪一步
//! 流失"需要 opt-in 遥测，那与本仓默认离线的姿态是另一件事，不在这里解决。把这条边界
//! 写在这儿，是为了下一个人不会指着这个文件说"漏斗已经有了"。
//!
//! 隐私：只写步骤 id 与时刻，**绝不含任何用户内容**（TDD-12 脱敏纪律）。Vault 路径、
//! 账户、模型名一律不进。

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use specta::Type;

/// 记录文件名（app_data_dir 下）。
pub(crate) const FUNNEL_FILE: &str = "onboarding-funnel.json";

/// 收场那一条用的伪步骤名。它不在 `ONBOARDING_STEP_IDS` 里，故不与任何真步骤撞名。
pub(crate) const COMPLETED_MARK: &str = "completed";

/// 一条记录：进了哪一步、什么时刻。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct FunnelMarkDto {
    /// 步骤 id，或收场标记 [`COMPLETED_MARK`]。
    pub step: String,
    /// Unix 毫秒。与 `VaultEntryDto::last_opened_ms` 同一口径（`f64`）——specta 禁止导出
    /// `i64`（怕精度丢失），而毫秒时刻约 1.8e12，远在 `f64` 的精确整数范围（2^53）之内。
    /// 用绝对时刻而不是相对偏移：本仓的设备级文件本就记绝对时刻，另造一套只会让两份
    /// 记录对不上。
    pub at_ms: f64,
}

/// 进程级记录器（`app.manage`）。
pub struct OnboardingFunnel {
    marks: Mutex<Vec<FunnelMarkDto>>,
    /// `None` = 无可写目录（测试 / 沙箱），只在内存里记。
    file: Option<PathBuf>,
}

impl OnboardingFunnel {
    /// 读回已有记录（跨启动续写：引导本来就可能分几次走完）。
    #[must_use]
    pub fn new(app_data_dir: Option<PathBuf>) -> Self {
        let file = app_data_dir.map(|dir| dir.join(FUNNEL_FILE));
        let marks = file
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<Vec<FunnelMarkDto>>(&bytes).ok())
            .unwrap_or_default();
        Self {
            marks: Mutex::new(marks),
            file,
        }
    }

    /// 记一条并落盘。**连续重复的同一步只记第一次**：壳层每次重渲染都可能报同一步，
    /// 而「反复进入同一屏」与「停在同一屏」在这份记录里是同一件事，记成几十条只会淹掉
    /// 真正的转移。
    pub fn enter(&self, step: &str, at_ms: f64) {
        let snapshot = {
            let Ok(mut marks) = self.marks.lock() else {
                return;
            };
            if marks.last().is_some_and(|last| last.step == step) {
                return;
            }
            marks.push(FunnelMarkDto {
                step: step.to_owned(),
                at_ms,
            });
            marks.clone()
        };
        self.persist(&snapshot);
    }

    fn persist(&self, marks: &[FunnelMarkDto]) {
        let Some(path) = &self.file else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(marks) {
            if let Err(error) = std::fs::write(path, bytes) {
                eprintln!("[onboarding] 写引导记录失败：{error}");
            }
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<FunnelMarkDto> {
        self.marks.lock().map(|m| m.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn funnel(dir: Option<PathBuf>) -> OnboardingFunnel {
        OnboardingFunnel::new(dir)
    }

    #[test]
    fn repeated_entries_into_the_same_step_collapse() {
        let f = funnel(None);
        f.enter("welcome", 1_000.0);
        f.enter("welcome", 1_200.0);
        f.enter("vault", 2_000.0);
        f.enter("welcome", 3_000.0);
        assert_eq!(
            f.snapshot()
                .iter()
                .map(|m| m.step.as_str())
                .collect::<Vec<_>>(),
            ["welcome", "vault", "welcome"],
            "只折叠**连续**重复；来回走属于真实转移，必须留下"
        );
    }

    /// 引导本来就可能分几次走完（中途退出应用再回来），故记录跨启动续写而不是覆盖。
    #[test]
    fn marks_survive_a_restart_and_keep_appending() {
        let dir = tempfile::tempdir().unwrap();
        let first = funnel(Some(dir.path().to_path_buf()));
        first.enter("welcome", 1_000.0);
        first.enter("vault", 2_000.0);

        let second = funnel(Some(dir.path().to_path_buf()));
        assert_eq!(second.snapshot().len(), 2);
        second.enter("compute", 3_000.0);
        assert_eq!(
            second
                .snapshot()
                .iter()
                .map(|m| m.step.as_str())
                .collect::<Vec<_>>(),
            ["welcome", "vault", "compute"]
        );
    }

    /// 无可写目录时只在内存里记，不报错、不 panic——测试与沙箱都走这条路。
    #[test]
    fn a_missing_data_dir_is_not_an_error() {
        let f = funnel(None);
        f.enter("welcome", 1_000.0);
        assert_eq!(f.snapshot().len(), 1);
    }
}
