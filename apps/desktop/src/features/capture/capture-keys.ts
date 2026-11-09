/**
 * 捕获输入框的键盘分类（纯函数，PRD-06-058 B / B″）。
 *
 * 组件只问「这一下是什么」，与 `popup-translate/popup-keys.ts` 同一形态。
 *
 * 「这一下属于输入法吗」不在本文件里判——那条规则两个功能面相同，唯一事实源是
 * `@lunarca/editor-core/ime-composition`。本文件只把「不是输入法的回车」翻译成捕获自己的动作。
 */

import {
  isImeCompositionKey,
  NO_COMPOSITION,
  type CompositionState,
  type ImeCompositionKey,
} from "@lunarca/editor-core/ime-composition";

export type CaptureKeyAction = "save" | "saveAndOpen" | "newline" | "ignore";

/** 分类所需的键盘事实（从 `KeyboardEvent` 现取，不含 DOM 依赖，便于单测）。 */
export interface CaptureKey extends ImeCompositionKey {
  key: string;
  shiftKey: boolean;
  metaKey: boolean;
  ctrlKey: boolean;
}

/**
 * - `save`：回车（无修饰）——存下并留在框里，连续捕获；
 * - `saveAndOpen`：⌘/Ctrl+回车——存下并打开；
 * - `newline`：Shift+回车——交给 textarea 的原生行为；
 * - `ignore`：非回车，或这一下属于输入法上屏。调用方必须连 `preventDefault` 一起放弃——
 *   吃掉上屏那一下，候选词就上不了屏。
 */
export function classifyCaptureKey(
  event: CaptureKey,
  composition: CompositionState = NO_COMPOSITION,
): CaptureKeyAction {
  if (event.key !== "Enter") {
    return "ignore";
  }
  if (isImeCompositionKey(event, composition)) {
    return "ignore";
  }
  if (event.shiftKey) {
    return "newline";
  }
  return event.metaKey || event.ctrlKey ? "saveAndOpen" : "save";
}
