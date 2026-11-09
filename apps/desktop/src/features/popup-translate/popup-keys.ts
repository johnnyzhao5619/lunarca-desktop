/**
 * 弹窗翻译输入框的键盘分类（纯函数，PRD-01-005）。
 *
 * 此前组件里只判 `Enter && !shiftKey`：Cmd/Ctrl+Enter 是顺带命中，Alt+Enter 也会触发生成，
 * 而**中文输入法合成态**按 Enter 上屏候选时同样命中——一句话没打完就被拿去翻译。
 * 分类收成一处后组件只问「这一下是什么」，不再自己拼修饰键。
 *
 * **「这一下属于输入法吗」不在本文件里判**：那条规则与捕获输入框（`capture/capture-keys.ts`）
 * 逐字相同，唯一事实源是 `@lunarca/editor-core/ime-composition`。本文件此前自带的
 * `isComposing || keyCode === 229` 是同一条规则的第二份定义，且**少了最要紧的第三条线索**：
 * WKWebView 里 `compositionend` 早于 `keydown`，上屏那一下身上 `isComposing = false`、
 * keyCode = 13，与用户敲的普通回车毫无区别——旧注释写的「WKWebView 可靠」已被 2026-09-09
 * 真机证伪，正好反了。补上的第三条是由组件记下的合成边界（见那边的运行时对照表）。
 */

import {
  isImeCompositionKey,
  NO_COMPOSITION,
  type CompositionState,
  type ImeCompositionKey,
} from "@lunarca/editor-core/ime-composition";

export type PopupInputKeyAction = "generate" | "newline" | "ignore";

export interface PopupInputKey extends ImeCompositionKey {
  key: string;
  shiftKey: boolean;
  altKey: boolean;
  metaKey: boolean;
  ctrlKey: boolean;
}

/**
 * - `generate`：Enter（无修饰）、Cmd+Enter、Ctrl+Enter——对标 DeepL / Bob 的零鼠标生成；
 * - `newline`：Shift+Enter、Alt+Enter——交给 textarea 默认行为；
 * - `ignore`：非 Enter，或这一下属于输入法（组字中 / 上屏候选那一下不是「生成」）。
 *   **`ignore` 与 `newline` 都必须原样放行**：吃掉上屏那一下，候选词就永远上不了屏。
 */
export function classifyPopupInputKey(
  event: PopupInputKey,
  composition: CompositionState = NO_COMPOSITION,
): PopupInputKeyAction {
  if (event.key !== "Enter") return "ignore";
  if (isImeCompositionKey(event, composition)) return "ignore";
  if (event.shiftKey || event.altKey) return "newline";
  return "generate";
}
