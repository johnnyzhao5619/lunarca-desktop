//! 启动前内存预检（ADR-143 ④）。
//!
//! Metal 缓冲是 wired 内存：超额不会让 llama-server 报错，而是挤到整机看门狗 panic
//! （P0-7：4B + 262 144 上下文 + f16 KV，wired 14.4 GB / 16 GB，91 s 无响应后重启）。
//! 那种故障发生后监管器根本没机会介入，所以防线只能在**加载之前**：
//!
//! - 「需要多少」问随包的 `llama-fit-params`：它与运行时是同一份估算代码、不分配内存、
//!   约 0.2 s，对 panic 那组参数估出 14 GB，与事故吻合；
//! - 「还剩多少」由调用方从系统内存统计给出——`llama-fit-params` 自己的「free」是
//!   Metal 的视角（16 GB 机器上恒约 12 GB），看不到其他进程，**不能当判据**。
//!
//! 放不下时按「KV 改 q8_0（速度无差，P0-4）→ 上下文减半」逐级退，到下限仍放不下即拒绝。

use super::preset::KvType;

/// 一次估算的结论：这组参数总共要多少内存（MiB，所有设备 + 主机的 `self` 之和）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Projection {
    /// 合计 MiB。
    pub total_mib: u64,
}

/// 从 `llama-fit-params -lv 4` 的输出里取**第一组**内存分解（初始参数下的估算）。
///
/// 行形如：
///
/// ```text
/// common_memory_breakdown_print: |   - MTL0 (Apple M5)    | 12124 = 12073 + (4440 =  3352 +     562 +     526) +       -4389 |
/// common_memory_breakdown_print: |   - Host               |                   569 =   497 +       0 +      72                |
/// ```
///
/// 设备行取括号里的 `self`，主机行取第一个数。找不到任何一行时返回 `None`
/// （输出格式变了——调用方必须当作「估不出来」而不是「零需求」）。
#[must_use]
pub fn parse_breakdown(output: &str) -> Option<Projection> {
    let mut total = 0u64;
    let mut rows = 0usize;
    let mut in_block = false;
    for line in output.lines() {
        let Some(rest) = line.split("common_memory_breakdown_print:").nth(1) else {
            if in_block {
                break;
            }
            continue;
        };
        let Some(row) = rest.trim_start().strip_prefix("|   - ") else {
            // 表头行：第二个表头意味着进入了第二组（fit 调整后的重估），只取第一组。
            if in_block && rows > 0 && rest.contains("memory breakdown") {
                break;
            }
            in_block = true;
            continue;
        };
        in_block = true;
        let cols: Vec<&str> = row.split('|').collect();
        let body = cols.get(1)?;
        let value = match body.split_once('(') {
            Some((_, inner)) => first_number(inner)?,
            None => first_number(body)?,
        };
        total = total.checked_add(value)?;
        rows += 1;
    }
    (rows > 0).then_some(Projection { total_mib: total })
}

fn first_number(text: &str) -> Option<u64> {
    text.split(|c: char| !c.is_ascii_digit())
        .find(|t| !t.is_empty())
        .and_then(|t| t.parse().ok())
}

/// 一个待加载模型的诉求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FitRequest {
    /// 目录条目 id（只用于结论回报）。
    pub id: String,
    /// 期望上下文（档位策略给的上限）。
    pub ctx: u32,
    /// 上下文下限：低于它这个模型就不值得加载（对话 4096，嵌入 = 分块上限对应的窗）。
    pub min_ctx: u32,
}

/// 预检结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FitVerdict {
    /// 放得下：按这组参数加载。`degraded` = 走了退让档（UI 可如实说明）。
    Fits {
        /// 定下的上下文。
        ctx: u32,
        /// 定下的 KV 类型。
        kv: KvType,
        /// 这组参数的估算占用。
        need_mib: u64,
        /// 是否低于期望（KV 退到 q8_0 或上下文被减）。
        degraded: bool,
    },
    /// 放不下：拒绝加载。`need_mib` 是下限参数的需求，`budget_mib` 是当时可给的额度。
    Refused {
        /// 下限参数下仍需要的内存。
        need_mib: u64,
        /// 可用额度（已扣余量与已排定的模型）。
        budget_mib: u64,
    },
    /// 估不出来（工具缺失 / 输出格式变化）：**按拒绝处理**，不赌。
    Unknown(String),
}

/// 估算器：给定上下文与 KV 类型，返回这组参数的内存需求。
pub trait Estimator {
    /// 估一次。
    ///
    /// # Errors
    /// 工具不可用或输出无法解析时返回原因。
    fn estimate(&self, id: &str, ctx: u32, kv: KvType) -> Result<Projection, String>;
}

/// 余量：扣在系统可用内存之上，给系统与用户其他应用留的空间。
/// 取 2 GiB 与物理内存 15% 的较大者（16 GB 机器约 2.4 GiB）。
#[must_use]
pub fn margin_mib(physical_mib: u64) -> u64 {
    (physical_mib * 15 / 100).max(2048)
}

/// 按优先级依次给一组模型定参数：前面的先占额度（DefaultLlm 先于 RealtimeLlm 先于嵌入，
/// 由调用方排序）。返回与 `requests` 一一对应的结论。
pub fn plan(
    available_mib: u64,
    margin: u64,
    requests: &[FitRequest],
    est: &dyn Estimator,
) -> Vec<FitVerdict> {
    let mut budget = available_mib.saturating_sub(margin);
    requests
        .iter()
        .map(|req| {
            let verdict = fit_one(budget, req, est);
            if let FitVerdict::Fits { need_mib, .. } = verdict {
                budget = budget.saturating_sub(need_mib);
            }
            verdict
        })
        .collect()
}

fn fit_one(budget: u64, req: &FitRequest, est: &dyn Estimator) -> FitVerdict {
    let floor = req.min_ctx.min(req.ctx).max(1);
    let mut ctx = req.ctx.max(floor);
    let mut last_need = 0;
    loop {
        for kv in [KvType::F16, KvType::Q8_0] {
            let need = match est.estimate(&req.id, ctx, kv) {
                Ok(p) => p.total_mib,
                Err(reason) => return FitVerdict::Unknown(reason),
            };
            if need <= budget {
                return FitVerdict::Fits {
                    ctx,
                    kv,
                    need_mib: need,
                    degraded: ctx < req.ctx || kv != KvType::F16,
                };
            }
            last_need = need;
        }
        if ctx == floor {
            return FitVerdict::Refused {
                need_mib: last_need,
                budget_mib: budget,
            };
        }
        ctx = (ctx / 2).max(floor);
    }
}

/// 用随包 `llama-fit-params` 估算（不分配内存，单次约 0.2 s）。
///
/// 只认调用方登记过的模型 id；参数与 preset 那一节一一对应——编码器嵌入要把批大小拉到
/// 整窗（否则估出的计算缓冲偏小），对话模型保持默认（拉大会把计算缓冲撑到数 GB）。
/// 嵌入模型不带 `--embeddings`：这个工具不认它，而池化方式不影响内存。
pub struct FitParamsEstimator {
    bin: std::path::PathBuf,
    models: std::collections::HashMap<String, (std::path::PathBuf, bool)>,
}

impl FitParamsEstimator {
    /// `bin` = `llama-fit-params` 的绝对路径（与 `llama-server` 同目录随包）。
    #[must_use]
    pub fn new(bin: std::path::PathBuf) -> Self {
        Self {
            bin,
            models: std::collections::HashMap::new(),
        }
    }

    /// 登记一个模型：`full_batch` = 编码器嵌入（批大小 = 上下文）。
    #[must_use]
    pub fn with_model(
        mut self,
        id: impl Into<String>,
        path: std::path::PathBuf,
        full_batch: bool,
    ) -> Self {
        self.models.insert(id.into(), (path, full_batch));
        self
    }
}

impl Estimator for FitParamsEstimator {
    fn estimate(&self, id: &str, ctx: u32, kv: KvType) -> Result<Projection, String> {
        let (path, full_batch) = self
            .models
            .get(id)
            .ok_or_else(|| format!("unknown model {id}"))?;
        let ctx_arg = ctx.to_string();
        let mut cmd = std::process::Command::new(&self.bin);
        cmd.arg("-m")
            .arg(path)
            .args([
                "-c",
                &ctx_arg,
                "-ctk",
                kv.as_arg(),
                "-ctv",
                kv.as_arg(),
                "-lv",
                "4",
            ])
            .stdin(std::process::Stdio::null());
        if *full_batch {
            cmd.args(["-b", &ctx_arg, "-ub", &ctx_arg]);
        }
        let out = cmd
            .output()
            .map_err(|e| format!("spawn llama-fit-params: {e}"))?;
        // 分解表打在日志流里（stderr），拟合结果打在 stdout；两边都扫。
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        );
        parse_breakdown(&text)
            .ok_or_else(|| "llama-fit-params printed no memory breakdown".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PANIC_PARAMS: &str = "\
0.00.039.941 I ggml_metal_device_init: GPU name:   MTL0 (Apple M5)
0.00.219.270 I common_memory_breakdown_print: | memory breakdown [MiB] | total    free    self   model   context   compute    unaccounted |
0.00.219.270 I common_memory_breakdown_print: |   - MTL0 (Apple M5)    | 12124 = 12073 + (12943 =  3352 +    8242 +    1349) +      -12893 |
0.00.219.270 I common_memory_breakdown_print: |   - Host               |                   1049 =   497 +       0 +     552                |
0.00.219.300 W common_fit_params: failed to fit params to free device memory: n_gpu_layers already set by user to 99, abort
";

    /// P0-7 事故那组参数（真实输出）：估出 13 992 MiB，与 panic 报告里 wired 14.4 GB 吻合。
    #[test]
    fn parses_the_panic_configuration() {
        assert_eq!(
            parse_breakdown(PANIC_PARAMS),
            Some(Projection {
                total_mib: 12_943 + 1049
            })
        );
    }

    #[test]
    fn takes_only_the_first_breakdown_block() {
        let two = format!(
            "{PANIC_PARAMS}I common_memory_breakdown_print: | memory breakdown [MiB] | total |\n\
             I common_memory_breakdown_print: |   - MTL0 (Apple M5)    | 12124 = 9000 + (100 = 1 + 2 + 3) + 0 |\n"
        );
        assert_eq!(parse_breakdown(&two).unwrap().total_mib, 13_992);
    }

    #[test]
    fn unparseable_output_is_none_not_zero() {
        assert_eq!(
            parse_breakdown("error: invalid argument: --embeddings\n"),
            None
        );
        assert_eq!(parse_breakdown(""), None);
    }

    /// 线性假模型：need = base + per_k_ctx × ctx/1024，q8_0 时上下文部分减半。
    struct Linear {
        base: u64,
        per_k_ctx: u64,
    }
    impl Estimator for Linear {
        fn estimate(&self, _id: &str, ctx: u32, kv: KvType) -> Result<Projection, String> {
            let kv_part = self.per_k_ctx * u64::from(ctx) / 1024;
            let kv_part = if kv == KvType::Q8_0 {
                kv_part / 2
            } else {
                kv_part
            };
            Ok(Projection {
                total_mib: self.base + kv_part,
            })
        }
    }

    fn req(ctx: u32) -> FitRequest {
        FitRequest {
            id: "m".into(),
            ctx,
            min_ctx: 4096,
        }
    }

    #[test]
    fn fits_at_full_settings_when_room() {
        let est = Linear {
            base: 3000,
            per_k_ctx: 32,
        };
        let v = plan(12_000, 2048, &[req(16_384)], &est);
        assert_eq!(
            v,
            vec![FitVerdict::Fits {
                ctx: 16_384,
                kv: KvType::F16,
                need_mib: 3512,
                degraded: false
            }]
        );
    }

    #[test]
    fn degrades_kv_before_context() {
        // f16 @16k = 3000+512 = 3512 > 3400；q8 @16k = 3256 ≤ 3400
        let est = Linear {
            base: 3000,
            per_k_ctx: 32,
        };
        let v = plan(3400, 0, &[req(16_384)], &est);
        assert_eq!(
            v,
            vec![FitVerdict::Fits {
                ctx: 16_384,
                kv: KvType::Q8_0,
                need_mib: 3256,
                degraded: true
            }]
        );
    }

    #[test]
    fn halves_context_down_to_floor_then_refuses() {
        let est = Linear {
            base: 3000,
            per_k_ctx: 512,
        };
        // 4096 f16 = 5048，q8 = 4024；预算 4100 → 落在 4096/q8
        let v = plan(4100, 0, &[req(16_384)], &est);
        assert!(matches!(
            v[0],
            FitVerdict::Fits {
                ctx: 4096,
                kv: KvType::Q8_0,
                ..
            }
        ));
        // 预算 3500：下限仍放不下 → 拒绝，报下限 q8 的需求
        let v = plan(3500, 0, &[req(16_384)], &est);
        assert_eq!(
            v,
            vec![FitVerdict::Refused {
                need_mib: 4024,
                budget_mib: 3500
            }]
        );
    }

    #[test]
    fn earlier_requests_consume_budget_for_later_ones() {
        let est = Linear {
            base: 3000,
            per_k_ctx: 0,
        };
        // 额度 6500 − 余量 1000 = 5500；第一个占 3000 后只剩 2500
        let v = plan(6500, 1000, &[req(8192), req(8192)], &est);
        assert!(matches!(v[0], FitVerdict::Fits { .. }));
        assert_eq!(
            v[1],
            FitVerdict::Refused {
                need_mib: 3000,
                budget_mib: 2500
            }
        );
    }

    #[test]
    fn estimator_failure_is_unknown_never_a_pass() {
        struct Broken;
        impl Estimator for Broken {
            fn estimate(&self, _: &str, _: u32, _: KvType) -> Result<Projection, String> {
                Err("llama-fit-params missing".into())
            }
        }
        assert_eq!(
            plan(64_000, 0, &[req(8192)], &Broken),
            vec![FitVerdict::Unknown("llama-fit-params missing".into())]
        );
    }

    #[test]
    fn margin_is_at_least_two_gib() {
        assert_eq!(margin_mib(8 * 1024), 2048);
        assert_eq!(margin_mib(16 * 1024), 2457);
    }
}
