// 实时预览里的嵌入 `![[…]]`（对标方案 D5）：独占一行的嵌入在光标不在这一行时整行换成渲染结果。
//
// 渲染不另写一份：部件里挂一个只含这一行的 `ReadingView`——图片、笔记卡片、可点的附件名、指不到
// 的提示与阅读面是同一份实现，解析同样问 Rust（`vault_resolve_embeds`）。夹在文字中间的嵌入
// 照原文显示（与阅读面「行内笔记嵌入显示成链接」同一个理由：一篇内容塞不进一行字里）。
//
// 点渲染结果的空白处：光标落到这一行末尾，整行还原成源码可编辑；点里面的按钮与链接照常生效。

import { createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { EditorState } from "@codemirror/state";
import { EditorView, WidgetType } from "@codemirror/view";
import type { SyntaxNode } from "@lezer/common";
import { ReadingView } from "./ReadingView";
import { remeasureOnResize, stopRemeasure } from "./widget-measure";

export interface EmbedPreviewOptions {
  notePath: () => string | null;
  onOpenPath: (path: string) => void;
  onActivateWikiLink: (target: string) => void;
}

/** 这个段落是不是只有一条嵌入（前后只有空白）：是就给出它的原文。 */
export function soleEmbedOf(state: EditorState, paragraph: SyntaxNode): string | null {
  if (paragraph.name !== "Paragraph") return null;
  const embed = paragraph.getChild("WikiEmbed");
  if (!embed) return null;
  const before = state.doc.sliceString(paragraph.from, embed.from);
  const after = state.doc.sliceString(embed.to, paragraph.to);
  return before.trim() === "" && after.trim() === "" ? state.doc.sliceString(embed.from, embed.to) : null;
}

const mounted = new WeakMap<HTMLElement, Root>();

export class EmbedWidget extends WidgetType {
  constructor(
    readonly source: string,
    readonly notePath: string,
    readonly lineEnd: number,
    readonly options: EmbedPreviewOptions,
  ) {
    super();
  }

  override eq(other: EmbedWidget) {
    return other.source === this.source && other.notePath === this.notePath;
  }

  toDOM(view: EditorView) {
    const element = document.createElement("div");
    element.className = "luna-editor-embed";
    element.contentEditable = "false";
    const root = createRoot(element);
    root.render(
      createElement(ReadingView, {
        content: this.source,
        notePath: this.notePath,
        onOpenPath: this.options.onOpenPath,
        onActivateWikiLink: this.options.onActivateWikiLink,
      }),
    );
    // 内容是异步解析出来的：高度一变就让编辑器重读行高，不然下面几行的点击位置会错。
    remeasureOnResize(view, element);
    mounted.set(element, root);
    element.addEventListener("mousedown", (event) => {
      if (event.target instanceof Element && event.target.closest("button, a, img")) return;
      event.preventDefault();
      // 这一行的末尾（部件之后的位置可能已随改动平移，按当前文档夹住）。
      view.dispatch({ selection: { anchor: Math.min(this.lineEnd, view.state.doc.length) } });
      view.focus();
    });
    return element;
  }

  override destroy(dom: HTMLElement) {
    const root = mounted.get(dom);
    mounted.delete(dom);
    stopRemeasure(dom);
    // 与代码块复制按钮同一做法：推到微任务后卸载 root。
    queueMicrotask(() => root?.unmount());
  }

  override ignoreEvent() {
    return true;
  }
}
