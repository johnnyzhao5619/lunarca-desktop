//! 确定性测试信号发生器（**仅 `cfg(test)` 编译，不进产物**）。
//!
//! # 为什么要有这个模块
//!
//! VAD 的门限、噪声底、时间常数全部以 **dBFS** 表达（见 [`crate::vad`]），可断言的用例
//! 因此必须能"点一个电平就造出一段信号"。这件事在 `vad` 与 `pipeline` 两处都要做——
//! 复制两份的下场不是多打几行字，而是**两份夹具的电平口径会悄悄分叉**（一处按幅度、
//! 一处按 RMS，差 3dB），于是同一个门限在两个文件里表现不同，而没有任何东西会变红。
//!
//! 两条硬纪律：
//! - **确定性**：自带 xorshift64\* 伪随机，不依赖 `rand` 的版本行为，同一颗种子在任何
//!   平台上逐比特相同——否则就是"某台机器上偶尔红"。
//! - **不依赖真机**：全部解析式/伪随机合成，headless CI 里跑。

/// xorshift64\* 伪随机源。
pub(crate) struct Rng(u64);

impl Rng {
    /// 以给定种子构造（种子恒置最低位为 1：xorshift 的全零状态是不动点）。
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// 均匀 (0, 1]——右开区间会让 `ln(u)` 取到 `-inf`，故取到 1 而不取到 0。
    fn next_unit(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 1.0) / (1u64 << 53) as f64
    }

    /// 标准正态（Box–Muller，只取一支：够用且实现最短，浪费一半熵无所谓）。
    pub(crate) fn next_gaussian(&mut self) -> f32 {
        let u1 = self.next_unit();
        let u2 = self.next_unit();
        ((-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()) as f32
    }
}

/// dBFS → 线性幅度。
pub(crate) fn lin(dbfs_val: f32) -> f32 {
    10f32.powf(dbfs_val / 20.0)
}

/// `n` 个高斯白噪样本，**总体** RMS = `rms_dbfs`。
///
/// 刻意**不**逐帧归一化到精确电平：底噪估计取的是最小值，帧间抖动（320 点帧的 RMS
/// 相对标准差约 4%，即 ±0.35dB）正是决定"底比标称低多少"的量，抹掉它等于把用例
/// 造成一个现实里不存在的理想情形。
pub(crate) fn noise(rng: &mut Rng, rms_dbfs: f32, n: usize) -> Vec<f32> {
    let sigma = lin(rms_dbfs);
    (0..n).map(|_| rng.next_gaussian() * sigma).collect()
}

/// `n` 个 300Hz 正弦样本，帧 RMS **精确**等于 `rms_dbfs`（`start` = 起始样本相位）。
///
/// 300Hz 是人声带内频率（去直流高通 80Hz 对它几乎无衰减，`normalizer` 那条链路必须用
/// 带内信号造底噪——直流常量会被整个滤掉，算出来是 −120dBFS 的封底值）。
/// 且 300Hz × 20ms = 恰好 6 个整周期 ⇒ 每个 VAD 帧内 `sin²` 均值精确为 0.5
/// ⇒ 帧 RMS = 幅度/√2，电平精确可控。
pub(crate) fn tone(rms_dbfs: f32, n: usize, start: usize) -> Vec<f32> {
    let amp = lin(rms_dbfs) * std::f32::consts::SQRT_2;
    (0..n)
        .map(|i| (2.0 * std::f32::consts::PI * 300.0 * (start + i) as f32 / 16_000.0).sin() * amp)
        .collect()
}
