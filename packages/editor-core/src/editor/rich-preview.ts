// 实时预览里的公式与 Mermaid 图表（对标方案 D1 / D2；Obsidian / Typora 同一形态）。
//
// - 行内公式 `$…$`：光标不在里面时换成排好的公式（ViewPlugin，只处理可见范围）；光标碰到、
//   或非空选区的端点落在里面时露出 TeX 源码——与粗体等行内标记同一条显隐规则（`live-preview.ts`）。
// - 成块公式 `$$…$$` 与 ```` ```mermaid ```` 代码块：光标不在这几行时整块换成渲染结果
//   （块级替换只能由 StateField 提供），光标进入即还原源码可编辑；点一下渲染结果也进入源码。
//   跨行的非空选区不还原（复制得到完整 Markdown，与表格同一裁决）。
// - 脚注引用 `[^id]`：光标外显示成上标编号，悬停给出脚注内容，点一下跳到定义（D3）。
// - 输入法组字期间不重建（A3 同一理由）：只把旧装饰随改动平移，组字结束再按最终文本重建。
// - 图表的画法归宿主（`MarkdownHost.diagrams`）：部件里挂的是阅读面同一个 `MermaidView`，内联 SVG 还是
//   宿主画好的图片、画不出来时怎么说，两面一个样。

import { createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { syntaxTree } from "@codemirror/language";
import { RangeSetBuilder, StateField, type EditorState, type Extension, type Range } from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin, WidgetType, type DecorationSet, type ViewUpdate } from "@codemirror/view";
import type { SyntaxNode } from "@lezer/common";
import { inlineToPlainText, parseInlineMarkdown } from "../markdown/markdown";
import { mathSource } from "../markdown/markdown-dialect";
import { renderMathInto } from "../render/math-render";
import { MermaidView } from "../render/MermaidView";
import type { EditorHost } from "./editor-host";
import { boundaryInside, caretTouches, revealsSyntax } from "./live-preview";
import { EmbedWidget, soleEmbedOf, type EmbedPreviewOptions } from "./embed-preview";
import { remeasureOnResize, stopRemeasure } from "./widget-measure";

export interface RichPreviewOptions {
  /** 嵌入 `![[…]]` 的渲染通道（D5）；缺省（还没落盘的笔记）不渲染嵌入。 */
  embed?: EmbedPreviewOptions;
}

class MathWidget extends WidgetType {
  constructor(
    readonly tex: string,
    readonly display: boolean,
    readonly from: number,
  ) {
    super();
  }

  override eq(other: MathWidget) {
    return other.tex === this.tex && other.display === this.display;
  }

  toDOM(view: EditorView) {
    const element = document.createElement(this.display ? "div" : "span");
    element.className = this.display ? "luna-math luna-math--block" : "luna-math";
    renderMathInto(element, this.tex, this.display);
    if (this.display) attachReveal(element, view, this.from);
    // KaTeX 是懒加载的：排好之前只是源码那么高。
    remeasureOnResize(view, element);
    return element;
  }

  override destroy(dom: HTMLElement) {
    stopRemeasure(dom);
  }

  override ignoreEvent() {
    return false;
  }
}

const diagramRoots = new WeakMap<HTMLElement, Root>();

class MermaidWidget extends WidgetType {
  constructor(
    readonly host: EditorHost,
    readonly code: string,
    readonly from: number,
  ) {
    super();
  }

  override eq(other: MermaidWidget) {
    return other.code === this.code;
  }

  toDOM(view: EditorView) {
    const element = document.createElement("div");
    const root = createRoot(element);
    root.render(createElement(MermaidView, { host: this.host.markdown, code: this.code }));
    diagramRoots.set(element, root);
    attachReveal(element, view, this.from);
    // 图是异步画出来的，换上图之后高度才定。
    remeasureOnResize(view, element);
    return element;
  }

  override destroy(dom: HTMLElement) {
    const root = diagramRoots.get(dom);
    diagramRoots.delete(dom);
    stopRemeasure(dom);
    // 与嵌入部件同一做法：CM 更新 DOM 时正处在事件处理里，推到微任务后再卸载 root。
    queueMicrotask(() => root?.unmount());
  }

  override ignoreEvent() {
    return false;
  }
}

/** 点一下渲染结果：光标落进源码（块随之还原），不必先找到它的源码在哪。 */
function attachReveal(element: HTMLElement, view: EditorView, from: number) {
  element.addEventListener("mousedown", (event) => {
    event.preventDefault();
    const line = view.state.doc.lineAt(Math.min(from, view.state.doc.length));
    const next = line.number < view.state.doc.lines ? view.state.doc.line(line.number + 1) : line;
    view.dispatch({ selection: { anchor: next.from } });
    view.focus();
  });
}

function blockLines(state: EditorState, node: SyntaxNode) {
  const last = state.doc.lineAt(Math.max(node.from, node.to - (state.doc.sliceString(node.to - 1, node.to) === "\n" ? 1 : 0)));
  return { from: state.doc.lineAt(node.from).from, to: last.to };
}

function isMermaid(state: EditorState, node: SyntaxNode): boolean {
  const info = node.getChild("CodeInfo");
  return info !== null && state.doc.sliceString(info.from, info.to).trim().toLowerCase() === "mermaid";
}

/** 光标（或单行内的选区）落在块里：还原源码。跨行选区不还原。 */
function blockTouched(state: EditorState, from: number, to: number): boolean {
  if (!revealsSyntax(state)) return false;
  return state.selection.ranges.some((range) => range.from <= to && range.to >= from);
}

function fenceCode(state: EditorState, node: SyntaxNode): string {
  const text = node.getChild("CodeText");
  return text ? state.doc.sliceString(text.from, text.to) : "";
}

export function buildRichBlockDecorations(host: EditorHost, state: EditorState, options: RichPreviewOptions): DecorationSet {
  const ranges: Range<Decoration>[] = [];
  const notePath = options.embed?.notePath() ?? null;
  for (let node = syntaxTree(state).topNode.firstChild; node; node = node.nextSibling) {
    const embed = options.embed && notePath ? soleEmbedOf(state, node) : null;
    if (embed !== null && options.embed && notePath) {
      const span = blockLines(state, node);
      if (!blockTouched(state, span.from, span.to)) {
        const widget = new EmbedWidget(host, embed, notePath, span.to, options.embed);
        ranges.push(Decoration.replace({ widget, block: true }).range(span.from, span.to));
      }
      continue;
    }
    const math = node.name === "BlockMath";
    const mermaid = node.name === "FencedCode" && isMermaid(state, node);
    if (!math && !mermaid) continue;
    const span = blockLines(state, node);
    if (blockTouched(state, span.from, span.to)) continue;
    const widget = math
      ? new MathWidget(mathSource(state.doc.sliceString(node.from, node.to), true), true, span.from)
      : new MermaidWidget(host, fenceCode(state, node), span.from);
    ranges.push(Decoration.replace({ widget, block: true }).range(span.from, span.to));
  }
  return Decoration.set(ranges, true);
}

/** 脚注引用：光标外显示成上标编号，悬停给出脚注内容，点一下跳到定义那一行。 */
class FootnoteRefWidget extends WidgetType {
  constructor(
    readonly id: string,
    readonly note: string,
    readonly definitionFrom: number,
  ) {
    super();
  }

  override eq(other: FootnoteRefWidget) {
    return other.id === this.id && other.note === this.note && other.definitionFrom === this.definitionFrom;
  }

  toDOM(view: EditorView) {
    const element = document.createElement("sup");
    element.className = "luna-footnote-ref";
    element.textContent = this.id;
    element.title = this.note;
    element.addEventListener("mousedown", (event) => {
      event.preventDefault();
      view.dispatch({ selection: { anchor: this.definitionFrom }, scrollIntoView: true });
      view.focus();
    });
    return element;
  }

  override ignoreEvent() {
    return false;
  }
}

/** 全篇的脚注定义：id → 定义起点与正文（文档直属块，不下钻）。 */
function footnoteDefinitions(state: EditorState): Map<string, { from: number; note: string }> {
  const out = new Map<string, { from: number; note: string }>();
  for (let node = syntaxTree(state).topNode.firstChild; node; node = node.nextSibling) {
    if (node.name !== "FootnoteDefinition") continue;
    const label = node.getChild("FootnoteLabel");
    const marks = node.getChildren("FootnoteMark");
    if (!label || marks.length < 2) continue;
    out.set(state.doc.sliceString(label.from, label.to), {
      from: node.from,
      // 与阅读面同一口径：去掉 Markdown 标记的纯文本。
      note: inlineToPlainText(parseInlineMarkdown(state.doc.sliceString(marks[1].to, node.to).replace(/\s+/gu, " ").trim())),
    });
  }
  return out;
}

function buildInlineMath(view: EditorView): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  const { state } = view;
  let definitions: Map<string, { from: number; note: string }> | null = null;
  for (const { from, to } of view.visibleRanges) {
    syntaxTree(state).iterate({
      from,
      to,
      enter(node) {
        if (node.name !== "InlineMath" && node.name !== "FootnoteRef") return;
        const touched = (revealsSyntax(state) && caretTouches(state, node.from, node.to)) || boundaryInside(state, node.from, node.to);
        if (touched) return false;
        if (node.name === "InlineMath") {
          const tex = mathSource(state.doc.sliceString(node.from, node.to), false);
          builder.add(node.from, node.to, Decoration.replace({ widget: new MathWidget(tex, false, node.from) }));
          return false;
        }
        const label = node.node.getChild("FootnoteLabel");
        const id = label ? state.doc.sliceString(label.from, label.to) : "";
        definitions ??= footnoteDefinitions(state);
        const definition = definitions.get(id);
        // 配不上定义的引用照原样显示：看起来像脚注、点下去什么也没有，比原样更糟。
        if (definition) {
          builder.add(node.from, node.to, Decoration.replace({ widget: new FootnoteRefWidget(id, definition.note, definition.from) }));
        }
        return false;
      },
    });
  }
  return builder.finish();
}

export function richPreviewExtension(host: EditorHost, options: RichPreviewOptions): Extension {
  const blocks = StateField.define<DecorationSet>({
    create: (state) => buildRichBlockDecorations(host, state, options),
    update(decos, tr) {
      const treeChanged = syntaxTree(tr.state) !== syntaxTree(tr.startState);
      if (!tr.docChanged && !tr.selection && !treeChanged) return decos;
      return buildRichBlockDecorations(host, tr.state, options);
    },
    provide: (f) => EditorView.decorations.from(f),
  });
  const inline = ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(view: EditorView) {
        this.decorations = buildInlineMath(view);
      }
      update(update: ViewUpdate) {
        if (update.view.composing) {
          if (update.docChanged) this.decorations = this.decorations.map(update.changes);
          return;
        }
        if (update.docChanged || update.viewportChanged || update.selectionSet || syntaxTree(update.state) !== syntaxTree(update.startState)) {
          this.decorations = buildInlineMath(update.view);
        }
      }
    },
    { decorations: (instance) => instance.decorations },
  );
  return [blocks, inline];
}
