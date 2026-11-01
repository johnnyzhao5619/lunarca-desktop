// 画出来之后才定高的部件（图片加载、公式 / 图表 / 嵌入异步渲染）：尺寸一变就让编辑器重读行高。
//
// CM 文档说「块部件高度变了就调 requestMeasure」，但 6.43 的 measure 只在 contentDOM 总高变了时
// 才重读各行高度；而 baseTheme 给 `.cm-content` 的 `min-height: 100%` 让短笔记的总高恒等于视口——
// 部件长高后行高表不更新，下面几行的点击按旧高度换算、落到文末（真机 2026-10-04，嵌入图片下方的
// 第一下点击总跑到最后一行）。置位 CM 内部的 `mustMeasureContent` 是唯一能强制重读的口子；
// 字段改名时 `widget-measure.test.ts` 会红。

import type { EditorView } from "@codemirror/view";

interface MeasureInternals {
  viewState?: { mustMeasureContent?: boolean | "refresh" };
}

/** 让编辑器在下一次测量里重读各行高度（不论 contentDOM 总高变没变）。 */
export function remeasureContent(view: EditorView): void {
  const viewState = (view as unknown as MeasureInternals).viewState;
  if (viewState) viewState.mustMeasureContent = true;
  view.requestMeasure();
}

const observers = new WeakMap<Element, ResizeObserver>();

/** 部件 DOM 尺寸一变就重读行高；部件 `destroy` 时调 `stopRemeasure` 收回。 */
export function remeasureOnResize(view: EditorView, element: HTMLElement): void {
  if (typeof ResizeObserver === "undefined") return;
  const observer = new ResizeObserver(() => remeasureContent(view));
  observer.observe(element);
  observers.set(element, observer);
}

export function stopRemeasure(element: HTMLElement): void {
  observers.get(element)?.disconnect();
  observers.delete(element);
}
