//! 弹窗翻译会话状态（`PopupSession`，决策 A / ADR-003 / PRD-01-016）。
//!
//! **缓存与源文本归 Rust、前端纯视图**：弹窗翻译的源文本来源（选区 / 双击读复制 / 手动 /
//! 语音）与会话内结果缓存全在此，前端只发命令 + 渲染（ADR-003）。这把原 `submit_popup_translate
//! (text, preset)`（前端持源文本）改为 **session 驱动的 `popup_generate(preset)`**：源文本从当前
//! 会话取，缓存命中即秒显（不重复跑 LLM，PRD-01-016）。
//!
//! **缓存策略（第一性）**：缓存**只存昂贵的 LLM 最终文本**，键 = `(源文本, preset_id)`；结果性质
//! 标识 `Meta`（译/润、目标语言、不确定性）每次由本地检测 + 分支**重算**——它确定、不调 LLM、
//! 极廉，存它只会引入失效风险。源文本一改即整体失效（PRD-01-016）：键含源文本，换文本自然
//! 不命中；并在 [`PopupSession::set_input`] 显式清空以释放内存。**关窗即清**（[`PopupSession::clear`]，
//! 不落盘，隐私优先）。
//!
//! **选区快照**（"替换原文"用，§1.4）随展示模式写入会话：替换流程必须基于触发时的源选区快照，
//! 而不是按钮点击后的全局焦点状态。点击弹窗按钮会改变 AX focus；此时再读“当前选区”会把源 App
//! 误判为失效。

use std::collections::HashMap;

const POPUP_ANCHOR_OFFSET: f64 = 12.0;

/// 弹窗焦点模式（TDD-05 §5.1.1，服务 PRD-01-005/006）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PopupMode {
    /// 展示模式：双击 / 选区触发，纯展示译文，面板不取键焦点。
    #[default]
    Display,
    /// 输入模式：手动 / 语音输入源，输入框按需取键焦点（不 activate App）。
    Input,
}

/// 触发弹窗时缓存的源选区快照（PRD-01-021 / TDD-05 §1.4）。
///
/// 文本用于脱敏校验；macOS 下同时封装触发瞬间的 AX 元素/range，确保点击弹窗按钮后不依赖全局焦点。
/// Windows 下记触发瞬间的前台窗口（注入前核对它仍在前台，ADR-163 ③）。
#[derive(Clone, Debug)]
pub struct SelectionSnapshot {
    source_text: String,
    #[cfg(target_os = "macos")]
    macos_target: Option<crate::platform::macos::selection::MacSelectionSnapshot>,
    #[cfg(target_os = "windows")]
    windows_target: Option<isize>,
}

impl SelectionSnapshot {
    #[must_use]
    pub fn new(source_text: String) -> Self {
        Self {
            source_text,
            #[cfg(target_os = "macos")]
            macos_target: None,
            #[cfg(target_os = "windows")]
            windows_target: None,
        }
    }

    /// Windows：带上触发瞬间的前台窗口（「替换原文」的注入目标）。
    #[cfg(target_os = "windows")]
    #[must_use]
    pub fn with_windows_target(source_text: String, windows_target: Option<isize>) -> Self {
        Self {
            source_text,
            windows_target,
        }
    }

    /// Windows：触发瞬间的前台窗口。
    #[cfg(target_os = "windows")]
    #[must_use]
    pub fn windows_target(&self) -> Option<isize> {
        self.windows_target
    }

    #[cfg(target_os = "macos")]
    #[must_use]
    pub fn with_macos_target(
        source_text: String,
        macos_target: crate::platform::macos::selection::MacSelectionSnapshot,
    ) -> Self {
        Self {
            source_text,
            macos_target: Some(macos_target),
        }
    }

    #[must_use]
    pub fn source_text(&self) -> &str {
        &self.source_text
    }

    #[cfg(target_os = "macos")]
    #[must_use]
    pub fn macos_target(&self) -> Option<&crate::platform::macos::selection::MacSelectionSnapshot> {
        self.macos_target.as_ref()
    }
}

impl PartialEq for SelectionSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.source_text == other.source_text
    }
}

impl Eq for SelectionSnapshot {}

/// “替换原文”按钮的纯决策结果。是否可尝试替换只取决于触发时是否有源选区快照；
/// 平台注入器负责在该快照上做 AX 直插 / 合成粘贴兜底。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplaceDecision {
    AttemptInjection,
    CopyFallback,
}

#[must_use]
pub fn decide_replace(snapshot: Option<&SelectionSnapshot>) -> ReplaceDecision {
    if snapshot.is_some() {
        ReplaceDecision::AttemptInjection
    } else {
        ReplaceDecision::CopyFallback
    }
}

/// 一次"开窗"的会话状态（单活动会话；关窗即清）。
#[derive(Debug, Default)]
pub struct PopupSession {
    /// 当前源文本（未设置前为 `None`——尚无可生成内容）。
    source_text: Option<String>,
    /// 当前焦点模式。
    mode: PopupMode,
    /// 触发时的源选区快照。"替换原文"只能在当前选区仍匹配该快照时执行，否则降级复制。
    selection_snapshot: Option<SelectionSnapshot>,
    /// 保存动作的归宿。仅由采集入口设定，避免前端根据文案或窗口标签猜测落盘目录。
    save_to_translate_space: bool,
    /// 输入模式的**未生成草稿**（PRD-01-005 修订 2026-09-07）：与源文本分开——它还没被拿去
    /// 翻译，不进缓存键；关窗 / 失焦收起都保留、重开预填；一旦成为源文本（`set_input`）即清。
    /// 此前输入只在「生成」时才写会话，而关窗清会话——打了半句按 Esc，重开是空的。
    draft: Option<String>,
    /// 这一次取选区借用了剪贴板、却没能把用户原来的内容原样放回（Windows 热键取选区，
    /// ADR-163 ②）。弹窗据此如实提示；每次捕获重新判定。
    clipboard_not_restored: bool,
    /// 会话内结果缓存：键 `(源文本, preset_id)` → LLM 最终译文。仅内存、关窗即清、不落盘。
    ///
    /// 注：MVP 每个 preset 的选项固定（`preset.options`），故 preset_id 已蕴含"关键选项"；
    /// 待引入每槽可调选项（如目标语言覆盖）时，键再并入选项指纹。
    cache: HashMap<(String, String), String>,
}

impl PopupSession {
    /// 设置输入模式的源文本（手动 / 语音 / 选区预填，PRD-01-005/006）。
    pub fn set_input(&mut self, text: String) {
        self.set_source(text, PopupMode::Input);
        self.selection_snapshot = None;
        // 草稿已升格为源文本。
        self.draft = None;
    }

    /// 写入未生成草稿；空串等于没有草稿。
    pub fn set_draft(&mut self, text: Option<String>) {
        self.draft = text.filter(|t| !t.is_empty());
    }

    /// 未生成草稿（克隆；`None` = 没有）。
    #[must_use]
    pub fn draft(&self) -> Option<String> {
        self.draft.clone()
    }

    /// 唤起空输入模式（备用热键 / 双击无选区，PRD-01-006）。
    ///
    /// 区别于 [`PopupSession::clear`]：这是一次有效开窗状态，模式为 `Input`；`clear` 是关窗清理，
    /// 回到默认展示态。
    pub fn set_empty_input(&mut self) {
        self.source_text = None;
        self.mode = PopupMode::Input;
        self.selection_snapshot = None;
        self.save_to_translate_space = false;
        self.clipboard_not_restored = false;
        self.cache.clear();
    }

    /// 记下这一次取选区没能原样还原剪贴板（在写入捕获结果之后调用）。
    pub fn mark_clipboard_not_restored(&mut self) {
        self.clipboard_not_restored = true;
    }

    /// 这一次取选区是否没能原样还原剪贴板。
    #[must_use]
    pub fn clipboard_not_restored(&self) -> bool {
        self.clipboard_not_restored
    }

    /// 设置展示模式的源文本（双击 / 选区触发捕获，§1.4 / PRD-01-024）。
    pub fn set_display(&mut self, text: String) {
        self.set_source(text, PopupMode::Display);
        self.selection_snapshot = None;
        self.save_to_translate_space = false;
    }

    /// 译境的 OCR 结果：保持与普通截图同一展示/翻译主干，但把用户确认的保存路由到专属目录。
    pub fn set_display_from_translate_space(&mut self, text: String) {
        self.set_source(text, PopupMode::Display);
        self.selection_snapshot = None;
        self.save_to_translate_space = true;
    }

    /// 设置展示模式源文本，并缓存触发时源选区快照（双击/选区路径）。
    pub fn set_display_with_snapshot(&mut self, text: String, snapshot: SelectionSnapshot) {
        self.set_source(text, PopupMode::Display);
        self.selection_snapshot = Some(snapshot);
        self.save_to_translate_space = false;
    }

    /// 置源文本 + 模式（DRY）。**源文本改变即整体失效**缓存（PRD-01-016）：清空已缓存结果。
    fn set_source(&mut self, text: String, mode: PopupMode) {
        if self.source_text.as_deref() != Some(text.as_str()) {
            self.cache.clear();
        }
        self.clipboard_not_restored = false;
        self.source_text = Some(text);
        self.mode = mode;
    }

    /// 当前源文本（克隆；`None` 表示尚无可生成内容）。
    #[must_use]
    pub fn source_text(&self) -> Option<String> {
        self.source_text.clone()
    }

    /// 当前焦点模式。
    #[must_use]
    pub fn mode(&self) -> PopupMode {
        self.mode
    }

    /// 当前源选区快照（克隆；正文不暴露给前端 IPC）。
    #[must_use]
    pub fn selection_snapshot(&self) -> Option<SelectionSnapshot> {
        self.selection_snapshot.clone()
    }

    #[must_use]
    pub fn save_to_translate_space(&self) -> bool {
        self.save_to_translate_space
    }

    /// 替换前校验：仅用于纯逻辑/回归测试。真实 IPC 替换不得在按钮点击后读取全局当前选区。
    #[must_use]
    pub fn can_replace_selection(&self, current_selected_text: Option<&str>) -> bool {
        self.selection_snapshot
            .as_ref()
            .is_some_and(|snapshot| current_selected_text == Some(snapshot.source_text()))
    }

    /// 查缓存：给定源文本 + preset 是否已生成过（命中即可秒显、不重复跑 LLM）。
    #[must_use]
    pub fn cached(&self, source_text: &str, preset_id: &str) -> Option<String> {
        self.cache
            .get(&(source_text.to_owned(), preset_id.to_owned()))
            .cloned()
    }

    /// 回填缓存（一次生成完成后存入最终译文）。
    pub fn put_cache(&mut self, source_text: String, preset_id: String, result: String) {
        self.cache.insert((source_text, preset_id), result);
    }

    /// 关窗清空（PRD-01-016：会话缓存关窗即清、不落盘）。**草稿保留**（PRD-01-005 修订）。
    pub fn clear(&mut self) {
        self.source_text = None;
        self.mode = PopupMode::default();
        self.selection_snapshot = None;
        self.clipboard_not_restored = false;
        self.cache.clear();
    }
}

/// 屏幕空间点（物理像素坐标）。用于把弹窗定位规则从 Tauri/AX API 中抽出成可单测纯逻辑。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenPoint {
    pub x: f64,
    pub y: f64,
}

impl ScreenPoint {
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// 屏幕空间尺寸（物理像素）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenSize {
    pub width: f64,
    pub height: f64,
}

impl ScreenSize {
    #[must_use]
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

/// 屏幕空间矩形（物理像素）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ScreenRect {
    #[must_use]
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    #[must_use]
    pub fn from_point(point: ScreenPoint) -> Self {
        Self::new(point.x, point.y, 0.0, 0.0)
    }

    fn center(self) -> ScreenPoint {
        ScreenPoint::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    fn contains(self, point: ScreenPoint) -> bool {
        point.x >= self.x
            && point.x <= self.x + self.width
            && point.y >= self.y
            && point.y <= self.y + self.height
    }

    /// 两个矩形是否有面积上的重叠（只贴边不算）。
    #[must_use]
    pub fn intersects(self, other: Self) -> bool {
        self.x < other.x + other.width
            && other.x < self.x + self.width
            && self.y < other.y + other.height
            && other.y < self.y + self.height
    }
}

/// 弹窗定位：优先贴近选区 bounds；无 bounds 时把 cursor 当作 0x0 anchor。
///
/// 多屏规则：用 anchor 中心点选择所在屏，再在该屏内 clamp，避免当前 `primary_monitor()`
/// 在副屏触发时把窗口夹回主屏。
#[must_use]
pub fn place_popup(anchor: ScreenRect, popup: ScreenSize, monitors: &[ScreenRect]) -> ScreenPoint {
    let is_point_anchor = anchor.width == 0.0 && anchor.height == 0.0;
    let raw_x = if is_point_anchor {
        anchor.x + POPUP_ANCHOR_OFFSET
    } else {
        anchor.x
    };
    let raw_y = anchor.y + anchor.height + POPUP_ANCHOR_OFFSET;
    clamp_to_monitor(
        ScreenPoint::new(raw_x, raw_y),
        popup,
        anchor.center(),
        monitors,
    )
}

/// HUD 面板定位（语音输入 / 快捷对话 / 录音 HUD 共用）：水平居中于光标、垂直在光标下方
/// `gap` 处，并 clamp 到光标所在屏内——**保证面板完整可见，绝不超出屏幕边缘**。
/// 多屏规则同 [`place_popup`]（按焦点所在屏 clamp，不夹回主屏）。
#[must_use]
pub fn place_panel_at_cursor(
    cursor: ScreenPoint,
    panel: ScreenSize,
    gap: f64,
    monitors: &[ScreenRect],
) -> ScreenPoint {
    let raw = ScreenPoint::new(cursor.x - panel.width / 2.0, cursor.y + gap);
    clamp_to_monitor(raw, panel, cursor, monitors)
}

/// 已存位置的 clamp（固定位置面板显示前校正）：屏幕布局变化（拔显示器/改分辨率）后，
/// 记忆坐标可能已在屏外——按记忆点所在屏（不在任何屏则回退首屏）夹回完整可见区。
#[must_use]
pub fn clamp_saved_position(
    saved: ScreenPoint,
    panel: ScreenSize,
    monitors: &[ScreenRect],
) -> ScreenPoint {
    clamp_to_monitor(saved, panel, saved, monitors)
}

/// 提词器「贴近镜头」的窄条（ADR-176 ⑤，PRD-20-008）：在一块显示器的**工作区**（物理像素，已扣掉
/// 菜单栏——带刘海的内建屏，工作区上沿就是刘海下沿）上沿居中，宽取工作区一半、夹在
/// [`CAMERA_STRIP_MIN_WIDTH`, `CAMERA_STRIP_MAX_WIDTH`] 逻辑点之间，高为 [`CAMERA_STRIP_HEIGHT`]。
/// 窄条让读的那几行贴着镜头、眼球横移小。`scale` 为显示器缩放（逻辑点 → 物理像素）。
#[must_use]
pub fn camera_strip_rect(work: ScreenRect, scale: f64) -> ScreenRect {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let width = (work.width / 2.0)
        .clamp(
            CAMERA_STRIP_MIN_WIDTH * scale,
            CAMERA_STRIP_MAX_WIDTH * scale,
        )
        .min(work.width);
    let height = (CAMERA_STRIP_HEIGHT * scale).min(work.height);
    ScreenRect::new(
        work.x + (work.width - width) / 2.0,
        work.y + CAMERA_STRIP_GAP * scale,
        width,
        height,
    )
}

/// 贴近镜头窄条的尺寸（逻辑点）：最窄与提词器面板的最小宽一致。
pub const CAMERA_STRIP_MIN_WIDTH: f64 = 420.0;
/// 贴近镜头窄条最宽（逻辑点）：再宽眼球横移就明显了。
pub const CAMERA_STRIP_MAX_WIDTH: f64 = 760.0;
/// 贴近镜头窄条的高（逻辑点）：控制条 + 三到四行大字。
pub const CAMERA_STRIP_HEIGHT: f64 = 260.0;
/// 窄条与工作区上沿的间隙（逻辑点）。
const CAMERA_STRIP_GAP: f64 = 4.0;

/// 记忆的窗口几何（位置 + 尺寸）在显示前校正（字幕窗记住上一次，PRD-04-040）：先按窗心所在
/// 屏（不在任何屏则首屏）把尺寸收进该屏——换到一块更小的屏时整窗仍放得下——再夹位置。
/// 无显示器信息时原样返回。
#[must_use]
pub fn clamp_saved_rect(saved: ScreenRect, monitors: &[ScreenRect]) -> ScreenRect {
    let Some(monitor) = monitor_containing(saved.center(), monitors) else {
        return saved;
    };
    let size = ScreenSize::new(
        saved.width.min(monitor.width),
        saved.height.min(monitor.height),
    );
    let pos = clamp_to_monitor(
        ScreenPoint::new(saved.x, saved.y),
        size,
        saved.center(),
        &[monitor],
    );
    ScreenRect::new(pos.x, pos.y, size.width, size.height)
}

/// [`stack_beside`] 给出的落点：左上角与（可能压矮了的）高度。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StackSlot {
    pub x: f64,
    pub y: f64,
    pub height: f64,
}

/// 尺寸为 `size` 的窗贴在 `anchor` 上方（优先）或下方、间隔 `gap`，水平与 `anchor` 同中线，
/// 以 `anchor` 所在屏的可用区为界（PRD-04-044 译文窗贴原文窗）。原高放不下时压矮到刚好
/// 放得下——常见的 900 点高屏上原文窗贴底，上方可能放不下一扇默认高的窗；压到
/// `min_height` 仍放不下才返回 None：宁可退回默认定位，也不给一扇挤到控件都被遮住的窗。
#[must_use]
pub fn stack_beside(
    anchor: ScreenRect,
    size: ScreenSize,
    monitors: &[ScreenRect],
    gap: f64,
    min_height: f64,
) -> Option<StackSlot> {
    let screen = monitor_containing(anchor.center(), monitors)?;
    let centered = anchor.x + (anchor.width - size.width) / 2.0;
    let x = centered.clamp(
        screen.x,
        (screen.x + screen.width - size.width).max(screen.x),
    );
    let room_above = anchor.y - gap - screen.y;
    let room_below = screen.y + screen.height - (anchor.y + anchor.height + gap);
    let fit = |room: f64| {
        let height = size.height.min(room.max(0.0));
        (height >= min_height.min(size.height)).then_some(height)
    };
    if let Some(height) = fit(room_above) {
        return Some(StackSlot {
            x,
            y: anchor.y - gap - height,
            height,
        });
    }
    fit(room_below).map(|height| StackSlot {
        x,
        y: anchor.y + anchor.height + gap,
        height,
    })
}

/// 包含 `point` 的屏，否则首屏；没有任何屏信息时 None。
fn monitor_containing(point: ScreenPoint, monitors: &[ScreenRect]) -> Option<ScreenRect> {
    monitors
        .iter()
        .copied()
        .find(|m| m.contains(point))
        .or_else(|| monitors.first().copied())
}

/// 把 `raw` 左上角 clamp 进「包含 `focus` 的屏」（否则首屏）内，使 `size` 完整可见。
/// 无显示器信息时原样返回（保守不动）。
fn clamp_to_monitor(
    raw: ScreenPoint,
    size: ScreenSize,
    focus: ScreenPoint,
    monitors: &[ScreenRect],
) -> ScreenPoint {
    let Some(monitor) = monitor_containing(focus, monitors) else {
        return raw;
    };
    let max_x = (monitor.x + monitor.width - size.width).max(monitor.x);
    let max_y = (monitor.y + monitor.height - size.height).max(monitor.y);
    ScreenPoint::new(raw.x.clamp(monitor.x, max_x), raw.y.clamp(monitor.y, max_y))
}

/// 输入模式失焦收起状态机。
///
/// 旧的 `Focused(false) -> close` 会在 show 初始化 blur、展示模式 hover/click 或事件时序抖动时误清
/// 会话。这里仅允许"输入模式 show 后，窗口曾真实获得 key/focus，再失焦"触发关闭。
///
/// **固定**（ADR-137 H4，对标 Bob / Easydict 的图钉）：用户要一边看译文一边在别的应用里
/// 打字时，失焦收起正是它要关掉的那件事。固定只管这一次唤起——下一次唤起重新武装，
/// 否则一个忘了取消的图钉会让「失焦即收」从此失效。
#[derive(Debug, Default)]
pub struct PopupBlurState {
    mode: PopupMode,
    became_key: bool,
    pinned: bool,
}

impl PopupBlurState {
    pub fn arm_for_show(&mut self, mode: PopupMode) {
        self.mode = mode;
        self.became_key = false;
        self.pinned = false;
    }

    pub fn set_pinned(&mut self, pinned: bool) {
        self.pinned = pinned;
    }

    #[must_use]
    pub fn on_focus_changed(&mut self, focused: bool) -> bool {
        if self.mode != PopupMode::Input || self.pinned {
            return false;
        }
        if focused {
            self.became_key = true;
            false
        } else {
            self.became_key
        }
    }
}

/// 双击复制命中后的捕获判定结果（TDD-05 §1.2 的"同一手势两种归宿"）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureKind {
    /// 有选区（`changeCount` 自增且读到非空文本）→ 展示模式翻译选区。
    DisplayWithSelection,
    /// 无选区（超时未自增，或读到空）→ 输入模式空窗。
    InputNoSelection,
}

/// **双击捕获判定（纯逻辑，可单测，TDD-05 §1.2）**：`changeCount` 是否自增正是"用户当下有无
/// 选区"的可靠信号——无选区时 `Cmd+C` 是 no-op、pasteboard 不被写、`changeCount` 不变。
///
/// 自增**且**读到非空文本 → 展示模式译选区；否则（超时未增 / 读到空白）→ 输入模式空窗。
/// 不读到旧值：仅当确认自增才取文本（调用方据此只在自增分支读取，避免读到上一次的剪贴板内容）。
#[must_use]
pub fn classify_capture(changecount_incremented: bool, captured_text: Option<&str>) -> CaptureKind {
    match captured_text {
        Some(t) if changecount_incremented && !t.trim().is_empty() => {
            CaptureKind::DisplayWithSelection
        }
        _ => CaptureKind::InputNoSelection,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_incremented_with_text_is_display() {
        assert_eq!(
            classify_capture(true, Some("hello")),
            CaptureKind::DisplayWithSelection
        );
    }

    #[test]
    fn classify_not_incremented_is_input_even_with_stale_text() {
        // 超时未自增：即使剪贴板有旧文本也判无选区（不读旧值，§1.2）。
        assert_eq!(
            classify_capture(false, Some("旧剪贴板")),
            CaptureKind::InputNoSelection
        );
    }

    #[test]
    fn classify_incremented_but_blank_is_input() {
        // 自增但读到空白（如复制到的是非文本）→ 输入模式。
        assert_eq!(
            classify_capture(true, Some("   ")),
            CaptureKind::InputNoSelection
        );
        assert_eq!(classify_capture(true, None), CaptureKind::InputNoSelection);
    }

    #[test]
    fn set_display_sets_text_and_display_mode() {
        let mut s = PopupSession::default();
        s.set_display("选中的英文".into());
        assert_eq!(s.source_text().as_deref(), Some("选中的英文"));
        assert_eq!(s.mode(), PopupMode::Display);
        assert!(!s.save_to_translate_space());
    }

    #[test]
    fn translate_space_display_routes_only_its_save_action_to_the_dedicated_folder() {
        let mut s = PopupSession::default();
        s.set_display_from_translate_space("captured text".into());
        assert!(s.save_to_translate_space());

        // 用户可以在弹窗中修正 OCR 原文，保存归宿不能因此丢失。
        s.set_input("corrected text".into());
        assert!(s.save_to_translate_space());

        // 新的普通会话必须明确回到通用保存路径。
        s.set_empty_input();
        assert!(!s.save_to_translate_space());
    }

    /// 「剪贴板没能还原」只属于那一次捕获：下一次捕获、关窗都清掉，不会挂到之后的唤起上。
    #[test]
    fn clipboard_notice_belongs_to_one_capture() {
        let mut s = PopupSession::default();
        assert!(!s.clipboard_not_restored());
        s.set_display_with_snapshot("hello".into(), SelectionSnapshot::new("hello".into()));
        s.mark_clipboard_not_restored();
        assert!(s.clipboard_not_restored());
        s.set_display_with_snapshot("again".into(), SelectionSnapshot::new("again".into()));
        assert!(!s.clipboard_not_restored(), "下一次捕获重新判定");
        s.mark_clipboard_not_restored();
        s.set_empty_input();
        assert!(!s.clipboard_not_restored());
        s.mark_clipboard_not_restored();
        s.clear();
        assert!(!s.clipboard_not_restored(), "关窗即清");
    }

    #[test]
    fn set_display_with_snapshot_keeps_source_selection() {
        let mut s = PopupSession::default();
        s.set_display_with_snapshot("hello".into(), SelectionSnapshot::new("hello".into()));

        assert_eq!(s.source_text().as_deref(), Some("hello"));
        assert_eq!(
            s.selection_snapshot().map(|x| x.source_text().to_string()),
            Some("hello".to_string())
        );
        assert!(s.can_replace_selection(Some("hello")));
    }

    #[test]
    fn replace_requires_current_selection_to_match_snapshot() {
        let mut s = PopupSession::default();
        s.set_display_with_snapshot("hello".into(), SelectionSnapshot::new("hello".into()));

        assert!(!s.can_replace_selection(Some("different")));
        assert!(!s.can_replace_selection(None));
    }

    #[test]
    fn replace_decision_uses_stored_snapshot_not_current_focus() {
        let snapshot = SelectionSnapshot::new("hello".into());

        assert_eq!(
            decide_replace(Some(&snapshot)),
            ReplaceDecision::AttemptInjection
        );
        assert_eq!(decide_replace(None), ReplaceDecision::CopyFallback);
    }

    #[test]
    fn set_input_sets_text_and_input_mode() {
        let mut s = PopupSession::default();
        s.set_display_with_snapshot("before".into(), SelectionSnapshot::new("before".into()));
        s.set_input("hello".into());
        assert_eq!(s.source_text().as_deref(), Some("hello"));
        assert_eq!(s.mode(), PopupMode::Input);
        assert_eq!(s.selection_snapshot(), None);
    }

    #[test]
    fn set_empty_input_sets_input_mode_without_source() {
        let mut s = PopupSession::default();
        s.set_display("hello".into());
        s.put_cache("hello".into(), "general".into(), "你好".into());

        s.set_empty_input();

        assert_eq!(s.source_text(), None);
        assert_eq!(s.mode(), PopupMode::Input);
        assert_eq!(s.selection_snapshot(), None);
        assert_eq!(s.cached("hello", "general"), None);
    }

    #[test]
    fn cache_hit_after_put() {
        let mut s = PopupSession::default();
        s.set_input("你好".into());
        assert_eq!(s.cached("你好", "general"), None);
        s.put_cache("你好".into(), "general".into(), "hello".into());
        assert_eq!(s.cached("你好", "general").as_deref(), Some("hello"));
        // 不同 preset 不串：
        assert_eq!(s.cached("你好", "slack"), None);
    }

    #[test]
    fn changing_source_text_invalidates_cache() {
        let mut s = PopupSession::default();
        s.set_input("你好".into());
        s.put_cache("你好".into(), "general".into(), "hello".into());
        // 改源文本 → 整体失效（PRD-01-016）。
        s.set_input("再见".into());
        assert_eq!(s.cached("你好", "general"), None);
        assert_eq!(s.cached("再见", "general"), None);
    }

    #[test]
    fn set_input_same_text_keeps_cache() {
        let mut s = PopupSession::default();
        s.set_input("你好".into());
        s.put_cache("你好".into(), "general".into(), "hello".into());
        // 重复设置相同源文本不应清缓存（避免无谓重算）。
        s.set_input("你好".into());
        assert_eq!(s.cached("你好", "general").as_deref(), Some("hello"));
    }

    #[test]
    fn clear_resets_everything() {
        let mut s = PopupSession::default();
        s.set_input("你好".into());
        s.put_cache("你好".into(), "general".into(), "hello".into());
        s.clear();
        assert_eq!(s.source_text(), None);
        assert_eq!(s.mode(), PopupMode::Display);
        assert_eq!(s.selection_snapshot(), None);
        assert_eq!(s.cached("你好", "general"), None);
    }

    #[test]
    fn input_blur_requires_a_real_key_focus_before_close() {
        let mut blur = PopupBlurState::default();
        blur.arm_for_show(PopupMode::Input);

        assert!(!blur.on_focus_changed(false));
        assert!(!blur.on_focus_changed(true));
        assert!(blur.on_focus_changed(false));
    }

    #[test]
    fn pinned_input_survives_blur_until_the_next_show() {
        let mut blur = PopupBlurState::default();
        blur.arm_for_show(PopupMode::Input);
        assert!(!blur.on_focus_changed(true));
        blur.set_pinned(true);
        assert!(!blur.on_focus_changed(false), "固定后失焦不收");
        blur.set_pinned(false);
        assert!(!blur.on_focus_changed(true));
        assert!(blur.on_focus_changed(false), "取消固定即恢复失焦收起");
        blur.set_pinned(true);
        blur.arm_for_show(PopupMode::Input);
        assert!(!blur.on_focus_changed(true));
        assert!(blur.on_focus_changed(false), "下一次唤起重新武装");
    }

    #[test]
    fn display_blur_never_closes_session() {
        let mut blur = PopupBlurState::default();
        blur.arm_for_show(PopupMode::Display);

        assert!(!blur.on_focus_changed(true));
        assert!(!blur.on_focus_changed(false));
    }

    #[test]
    fn placing_popup_uses_anchor_monitor_instead_of_primary_monitor() {
        let monitors = [
            ScreenRect::new(0.0, 0.0, 1440.0, 900.0),
            ScreenRect::new(1440.0, 0.0, 1440.0, 900.0),
        ];
        let anchor = ScreenRect::new(2500.0, 100.0, 20.0, 20.0);
        let popup = ScreenSize::new(440.0, 360.0);

        let pos = place_popup(anchor, popup, &monitors);

        assert!(pos.x >= 1440.0);
        assert_eq!(pos.y, 132.0);
    }

    #[test]
    fn placing_popup_clamps_inside_selected_monitor() {
        let monitors = [ScreenRect::new(1440.0, 0.0, 1440.0, 900.0)];
        let anchor = ScreenRect::new(2820.0, 860.0, 20.0, 20.0);
        let popup = ScreenSize::new(440.0, 360.0);

        let pos = place_popup(anchor, popup, &monitors);

        assert_eq!(pos.x, 2440.0);
        assert_eq!(pos.y, 540.0);
    }

    #[test]
    fn panel_at_cursor_centers_horizontally_below_cursor() {
        let monitors = [ScreenRect::new(0.0, 0.0, 1440.0, 900.0)];
        let pos = place_panel_at_cursor(
            ScreenPoint::new(700.0, 300.0),
            ScreenSize::new(400.0, 200.0),
            24.0,
            &monitors,
        );
        assert_eq!(pos, ScreenPoint::new(500.0, 324.0));
    }

    #[test]
    fn panel_at_cursor_near_edges_stays_fully_on_screen() {
        let monitors = [ScreenRect::new(0.0, 0.0, 1440.0, 900.0)];
        let panel = ScreenSize::new(400.0, 300.0);
        // 屏幕底部触发：面板须完整可见（不被夹出下边缘）。
        let bottom = place_panel_at_cursor(ScreenPoint::new(1400.0, 880.0), panel, 24.0, &monitors);
        assert_eq!(bottom, ScreenPoint::new(1040.0, 600.0));
        // 屏幕左上角触发：不越出左/上边缘。
        let corner = place_panel_at_cursor(ScreenPoint::new(10.0, 5.0), panel, 24.0, &monitors);
        assert_eq!(corner, ScreenPoint::new(0.0, 29.0));
    }

    #[test]
    fn saved_position_is_clamped_back_after_display_layout_change() {
        let monitors = [ScreenRect::new(0.0, 0.0, 1440.0, 900.0)];
        let panel = ScreenSize::new(400.0, 200.0);
        // 记忆坐标在已拔除的副屏上 → 夹回现有首屏完整可见区。
        let pos = clamp_saved_position(ScreenPoint::new(2000.0, 1200.0), panel, &monitors);
        assert_eq!(pos, ScreenPoint::new(1040.0, 700.0));
        // 记忆坐标仍在屏内 → 原样保留（不打扰用户的固定位置）。
        let kept = clamp_saved_position(ScreenPoint::new(300.0, 400.0), panel, &monitors);
        assert_eq!(kept, ScreenPoint::new(300.0, 400.0));
    }
}

#[cfg(test)]
mod draft_tests {
    use super::*;

    /// 草稿（PRD-01-005 修订）：关窗 / 备用热键重开都保留；一旦生成（`set_input`）即清；
    /// 空串等于没有草稿。
    #[test]
    fn draft_survives_clear_and_empty_input_but_not_set_input() {
        let mut s = PopupSession::default();
        s.set_draft(Some("half a sent".into()));
        s.clear();
        assert_eq!(s.draft().as_deref(), Some("half a sent"), "关窗不丢草稿");
        s.set_empty_input();
        assert_eq!(
            s.draft().as_deref(),
            Some("half a sent"),
            "热键重开不丢草稿"
        );
        s.set_input("half a sentence".into());
        assert!(s.draft().is_none(), "生成即清草稿");
        s.set_draft(Some(String::new()));
        assert!(s.draft().is_none(), "空串不算草稿");
    }
}

#[cfg(test)]
mod caption_window_geometry_tests {
    use super::{clamp_saved_rect, stack_beside, ScreenRect, ScreenSize, StackSlot};

    /// 1440×900 屏（可用区从菜单栏下沿 25 起）；原文窗 820×280 贴底居中。
    const SCREEN: ScreenRect = ScreenRect {
        x: 0.0,
        y: 25.0,
        width: 1440.0,
        height: 875.0,
    };
    const SOURCE: ScreenRect = ScreenRect {
        x: 310.0,
        y: 508.0,
        width: 820.0,
        height: 280.0,
    };
    const SIZE: ScreenSize = ScreenSize {
        width: 820.0,
        height: 280.0,
    };

    fn placed(slot: StackSlot, width: f64) -> ScreenRect {
        ScreenRect::new(slot.x, slot.y, width, slot.height)
    }

    #[test]
    fn the_translation_window_sits_just_above_the_source_window_at_its_own_height() {
        let slot = stack_beside(SOURCE, SIZE, &[SCREEN], 8.0, 200.0).expect("上方放得下");
        assert_eq!(
            slot,
            StackSlot {
                x: 310.0,
                y: 508.0 - 8.0 - 280.0,
                height: 280.0
            }
        );
        assert!(!placed(slot, 820.0).intersects(SOURCE));
    }

    #[test]
    fn a_tall_window_is_shortened_to_fit_above_without_reaching_under_the_menu_bar() {
        // 真机 2026-10-02 的情形：两扇都是 420 高，上方放不下原高。此前退回按屏高比例放，压住原文窗大半。
        let source = ScreenRect::new(310.0, 368.0, 820.0, 420.0);
        let slot = stack_beside(source, ScreenSize::new(820.0, 420.0), &[SCREEN], 8.0, 200.0)
            .expect("压矮后放得下");
        assert_eq!(slot.y, SCREEN.y, "贴到可用区顶，不钻进菜单栏");
        assert_eq!(slot.height, 368.0 - 8.0 - SCREEN.y);
        assert!(!placed(slot, 820.0).intersects(source));
    }

    #[test]
    fn falls_below_when_the_source_window_is_near_the_top() {
        let source = ScreenRect { y: 60.0, ..SOURCE };
        let slot = stack_beside(source, SIZE, &[SCREEN], 8.0, 200.0).expect("下方放得下");
        assert_eq!(slot.y, 60.0 + 280.0 + 8.0);
    }

    #[test]
    fn gives_up_rather_than_squeezing_below_the_minimum_height() {
        let source = ScreenRect::new(310.0, 200.0, 820.0, 560.0);
        assert_eq!(stack_beside(source, SIZE, &[SCREEN], 8.0, 200.0), None);
    }

    #[test]
    fn stays_on_screen_horizontally_when_the_source_window_hugs_an_edge() {
        let source = ScreenRect::new(1000.0, 600.0, 400.0, 200.0);
        let slot = stack_beside(source, SIZE, &[SCREEN], 8.0, 200.0).expect("上方放得下");
        assert_eq!(slot.x, 1440.0 - 820.0, "右缘不出屏");
    }

    #[test]
    fn a_remembered_window_comes_back_where_it_was() {
        let saved = ScreenRect::new(200.0, 120.0, 700.0, 240.0);
        assert_eq!(clamp_saved_rect(saved, &[SCREEN]), saved);
    }

    #[test]
    fn a_remembered_window_is_pulled_back_and_shrunk_when_the_screen_got_smaller() {
        // 记的时候在一块大屏上，现在只剩笔记本屏：整窗仍要完整可见。
        let saved = ScreenRect::new(1300.0, 700.0, 1600.0, 400.0);
        let back = clamp_saved_rect(saved, &[SCREEN]);
        assert_eq!(back.width, 1440.0);
        assert_eq!((back.x, back.y), (0.0, 25.0 + 875.0 - 400.0));
    }
}

#[cfg(test)]
mod camera_strip_tests {
    use super::*;

    #[test]
    fn the_strip_sits_centered_on_the_top_of_the_work_area() {
        // 14 寸 MacBook（2x）：工作区 3024×1890 物理像素、上沿已扣掉刘海那条菜单栏（y = 74）。
        let work = ScreenRect::new(0.0, 74.0, 3024.0, 1890.0);
        let rect = camera_strip_rect(work, 2.0);
        assert!((rect.width - 1512.0).abs() < 1e-9, "一半宽，未超上限 760×2");
        assert!(
            (rect.x + rect.width / 2.0 - 1512.0).abs() < 1e-9,
            "水平居中"
        );
        assert!((rect.y - (74.0 + 8.0)).abs() < 1e-9, "贴着工作区上沿");
        assert!((rect.height - 520.0).abs() < 1e-9);
    }

    #[test]
    fn the_strip_width_is_clamped_and_never_exceeds_the_screen() {
        let wide = camera_strip_rect(ScreenRect::new(-2560.0, 25.0, 5120.0, 1415.0), 1.0);
        assert!(
            (wide.width - CAMERA_STRIP_MAX_WIDTH).abs() < 1e-9,
            "宽屏夹到上限"
        );
        assert!(
            (wide.x + wide.width / 2.0).abs() < 1e-9,
            "副屏（负坐标）同样居中"
        );
        let narrow = camera_strip_rect(ScreenRect::new(0.0, 0.0, 600.0, 200.0), 1.0);
        assert!(
            (narrow.width - CAMERA_STRIP_MIN_WIDTH).abs() < 1e-9,
            "窄屏夹到下限"
        );
        assert!(narrow.height <= 200.0, "不比工作区还高");
        let bad_scale = camera_strip_rect(ScreenRect::new(0.0, 0.0, 1000.0, 800.0), f64::NAN);
        assert!(bad_scale.width.is_finite() && bad_scale.width > 0.0);
    }
}
