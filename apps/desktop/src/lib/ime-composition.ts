/**
 * 「这一下 keydown 属于输入法，不属于用户」的唯一判据（纯函数，无 DOM 依赖）。
 *
 * 消费者：捕获输入框（`features/capture/capture-keys.ts`）、弹窗翻译输入框
 * （`features/popup-translate/popup-keys.ts`），以及经 `lib/ime-key-event.ts` 文档级接线的
 * 其余「回车即动作」输入框与对话框（PRD-07-040 ①）。动作各不相同，这条规则相同。
 *
 * 放在 `lib/` 而非由某个 feature 导出：它属于「带输入框的键盘契约」这一层，
 * 让一个 feature 从另一个取，既锁错了归属，也会在两边出现反向 import 时撞上
 * 依赖环门禁。
 *
 * 只判 `isComposing` 不够——按回车上屏时三种运行时给出的事实不同：
 *
 * | 运行时 | 事件顺序 | keydown 上的事实 |
 * | --- | --- | --- |
 * | Chromium / Gecko | keydown → compositionend | isComposing = true（keyCode 229） |
 * | Windows 部分输入法 | 同上 | isComposing 可能为假，但 keyCode = 229 |
 * | WKWebView | compositionend → keydown | isComposing = false、keyCode = 13 |
 *
 * 最后一行是 2026-09-09 真机上发生的事：合成在 keydown 之前就结束，那一下与普通回车
 * 毫无区别，于是上屏被当成提交，一段拼音被落成笔记。事件字段区分不了它，只能由组件
 * 记下合成边界——`compositionend` 与其后的 keydown 出自同一次物理按键，时间戳几乎相等。
 *
 * 调用方拿到「属于输入法」之后必须原样放行：`preventDefault()` 吃掉这一下，候选词就
 * 永远上不了屏。判「这一下是什么」与「要不要拦下它」必须是同一个判断。
 *
 * 覆盖见 `capture-keys.test.ts` / `popup-keys.test.ts`：三种运行时各一条、时间窗两侧
 * 边界、以及「上屏之后再按一次照常执行」的反向条款。
 */

/**
 * 认定「这一下 keydown 属于刚刚那次上屏」的时间窗（毫秒）。
 *
 * 只用来认「同一次按键」，不用来猜意图：远小于人类再按一次回车的最短间隔（150ms 量级），
 * 也远大于同一次事件派发的抖动。窗外的回车一律照常执行调用方的动作。
 */
export const COMPOSITION_COMMIT_WINDOW_MS = 30;

/** 判据所需的键盘事实（从 `KeyboardEvent` 现取，便于单测）。 */
export interface ImeCompositionKey {
  /**
   * `KeyboardEvent.isComposing`。必须从 `nativeEvent` 取——React 合成事件不复制这个
   * 字段，传合成事件进来这条线索恒为 `undefined`。
   */
  isComposing?: boolean;
  /** 被输入法吃掉的键在多数运行时报 229（Windows 部分输入法只有这一条线索）。 */
  keyCode?: number;
  /** 事件时间戳（与 `compositionend` 同一时间原点）。 */
  timeStamp: number;
}

/** 组件记下的合成态。 */
export interface CompositionState {
  /** 此刻正在组字（`compositionstart` 之后、`compositionend` 之前）。 */
  active: boolean;
  /** 最近一次 `compositionend` 的时间戳；从未组过字时为 `Number.NEGATIVE_INFINITY`。 */
  endedAt: number;
}

/** 从未组过字的初值。`endedAt` 取负无穷，任何时间窗判定都不会误命中。 */
export const NO_COMPOSITION: CompositionState = {
  active: false,
  endedAt: Number.NEGATIVE_INFINITY,
};

/**
 * 这一下 keydown 是否属于输入法（组字中，或候选词上屏的那一下）。
 *
 * 四条线索并联，任一命中即属于输入法；缺哪一条会在哪种运行时失守，见文件头的表。
 */
export function isImeCompositionKey(
  event: ImeCompositionKey,
  composition: CompositionState = NO_COMPOSITION,
): boolean {
  if (event.isComposing || event.keyCode === 229 || composition.active) {
    return true;
  }
  // WKWebView：合成已在这一下之前结束，只有时间戳能证明它们是同一次按键。
  return event.timeStamp - composition.endedAt <= COMPOSITION_COMMIT_WINDOW_MS;
}
