/**
 * 「这一下 keydown 属于输入法吗」的文档级接线（PRD-07-040 ①）。
 *
 * 判据只在 `@lunarca/editor-core/ime-composition`；本文件只替调用方记下合成边界——WKWebView 的
 * `compositionend` 早于 `keydown`，上屏那一下的事件字段与普通回车无从区分，只能靠
 * 「刚刚结束过一次合成」这条时间线索（时序表见 `@lunarca/editor-core/ime-composition` 文件头）。
 *
 * 边界记在**文档**上而不是每个输入框上：合成事件会冒泡，在捕获阶段记一次，任何
 * keydown 处理（输入框自己的回车、列表的方向键、对话框挂在 `document` 上的 Esc）都能
 * 问同一个问题。逐个输入框记的话，对话框的 Esc 永远不知道输入框里正在组字，按 Esc
 * 取消候选会连对话框一起关掉。
 *
 * 「正在组字」只在**发起合成的那个元素仍是焦点**时成立：合成中途元素被卸载或失焦时
 * 可能收不到 `compositionend`，若不这样收口，全应用的回车与 Esc 都会被永久判给输入法。
 */

import { isImeCompositionKey, type CompositionState } from "@lunarca/editor-core/ime-composition";

interface DocumentComposition {
  target: EventTarget | null;
  endedAt: number;
}

const tracked = new WeakMap<Document, DocumentComposition>();

/** 在该文档上装合成边界监听（幂等）。模块加载时对当前文档自动装一次。 */
export function trackDocumentComposition(doc: Document): void {
  if (tracked.has(doc)) {
    return;
  }
  const state: DocumentComposition = { target: null, endedAt: Number.NEGATIVE_INFINITY };
  tracked.set(doc, state);
  doc.addEventListener(
    "compositionstart",
    (event) => {
      state.target = event.target;
    },
    true,
  );
  doc.addEventListener(
    "compositionend",
    (event) => {
      state.target = null;
      state.endedAt = event.timeStamp;
    },
    true,
  );
}

function compositionOf(doc: Document): CompositionState {
  const state = tracked.get(doc);
  if (!state) {
    return { active: false, endedAt: Number.NEGATIVE_INFINITY };
  }
  return {
    active: state.target !== null && state.target === doc.activeElement,
    endedAt: state.endedAt,
  };
}

/** 判据所需的键盘事件：原生事件或 React 合成事件（`isComposing` 只在原生事件上）。 */
type AnyKeyboardEvent = KeyboardEvent | { nativeEvent: KeyboardEvent };

/**
 * 这一下 keydown 属于输入法（组字中，或候选词上屏 / 取消的那一下）。
 *
 * 为真时调用方必须**整个放弃**这一下——尤其不能 `preventDefault`：吃掉它，候选词就上
 * 不了屏。
 */
export function isImeKeyEvent(event: AnyKeyboardEvent): boolean {
  const native = "nativeEvent" in event ? event.nativeEvent : event;
  const target = native.target;
  const doc = target instanceof Node ? (target.ownerDocument ?? (target as Document)) : globalThis.document;
  return isImeCompositionKey(native, doc ? compositionOf(doc) : undefined);
}

if (typeof document !== "undefined") {
  trackDocumentComposition(document);
}
