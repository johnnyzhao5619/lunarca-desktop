// 代码块整块容器化（PRD-06-011，Typora 对标）：给围栏/缩进代码块的**每一行**
// 加行级装饰 class（含空行与 fence 行），CSS 以整块底色 + 首末行圆角呈现——
// 取代「逐行胶囊」的破碎观感；块内文本的行内代码胶囊底色由 CSS 归零。
// 行级 Decoration.line 可由 ViewPlugin 提供（非块级 replace，无 StateField 约束）。
//
// 同一个块容器上还挂着「复制代码」按钮（首行右缘，绝对定位不占行盒）：阅读面早有
// 语言标签条 + 复制按钮，编辑面此前**没有任何复制入口**。按钮本体是阅读面同一个
// React 组件（宿主端口 `MarkdownHost.CodeCopyButton`，阅读面取的也是它），widget 只负责给它一个挂载点——
// 于是"复制走哪条命令、成功反馈多久、读屏念什么"仍然只有一处定义（TDD-15 §3.4a）。

import { createElement, type ComponentType } from "react";
import { createRoot, type Root } from "react-dom/client";
import { syntaxTree } from "@codemirror/language";
import { RangeSetBuilder, type EditorState, type Extension } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  ViewPlugin,
  WidgetType,
  type ViewUpdate,
} from "@codemirror/view";
import { codeBlockContentOf } from "../markdown/markdown";
import type { EditorHost } from "./editor-host";

/** 代码块「复制」按钮（宿主的 React 组件，与阅读面代码块头条是同一颗）。 */
type CopyButton = ComponentType<{ code: string }>;

interface Range {
  from: number;
  to: number;
}

const CODE_BLOCK_NODES = new Set(["FencedCode", "CodeBlock"]);

/**
 * 代码块首行右缘的「复制」按钮挂载点。
 *
 * widget 只是宿主：内容由 React 渲染，故它带自己的 root 并在 `destroy` 时卸载。
 * `eq` 按代码正文判等——正文没变就复用同一个 DOM，不会因为每次按键重挂一次 root
 * （那会把刚点出来的「✓」反馈闪掉）。
 */
class CodeCopyWidget extends WidgetType {
  private root: Root | null = null;

  constructor(
    private readonly code: string,
    private readonly button: CopyButton,
  ) {
    super();
  }

  override eq(other: CodeCopyWidget) {
    return other.code === this.code && other.button === this.button;
  }

  toDOM() {
    const host = document.createElement("span");
    host.className = "luna-md-code-copy";
    // 挂载点本身不是可编辑文本：不让光标落进按钮里。
    host.contentEditable = "false";
    this.root = createRoot(host);
    this.root.render(createElement(this.button, { code: this.code }));
    return host;
  }

  override destroy() {
    const root = this.root;
    this.root = null;
    // React 禁止在渲染周期内同步卸载 root（CM6 的 DOM 更新正处在事件处理里）；
    // 推到微任务后卸载是官方推荐的做法。
    queueMicrotask(() => root?.unmount());
  }
}

/** 构建代码块行装饰（state 级纯函数，可无头测试）。 */
export function buildCodeBlockLineDecorations(
  state: EditorState,
  ranges: readonly Range[],
  copyButton: CopyButton,
): DecorationSet {
  const lineNumbers = new Map<number, { first: boolean; last: boolean }>();
  /** 首行行号 → 该块的代码正文（复制按钮挂载点；空块不挂，复制空字符串没有意义）。 */
  const copyTargets = new Map<number, string>();
  for (const range of ranges) {
    syntaxTree(state).iterate({
      from: range.from,
      to: range.to,
      enter(node) {
        if (!CODE_BLOCK_NODES.has(node.name)) {
          return;
        }
        const first = state.doc.lineAt(node.from).number;
        const last = state.doc.lineAt(node.to).number;
        for (let n = first; n <= last; n++) {
          const existing = lineNumbers.get(n);
          lineNumbers.set(n, {
            first: (existing?.first ?? false) || n === first,
            last: (existing?.last ?? false) || n === last,
          });
        }
        // 「哪几行算这块代码」与阅读面同一个判断（`codeBlockContentOf`），否则同一块
        // 代码两面复制出的内容会差一行围栏或一层缩进。
        const { code } = codeBlockContentOf(node.node, (from, to) =>
          state.doc.sliceString(from, to),
        );
        if (code.trim() !== "") {
          copyTargets.set(first, code);
        }
      },
    });
  }
  const builder = new RangeSetBuilder<Decoration>();
  for (const n of [...lineNumbers.keys()].sort((a, b) => a - b)) {
    const { first, last } = lineNumbers.get(n)!;
    const cls = [
      "luna-md-codeblock-line",
      first ? "luna-md-codeblock-line--first" : "",
      last ? "luna-md-codeblock-line--last" : "",
    ]
      .filter(Boolean)
      .join(" ");
    const line = state.doc.line(n);
    builder.add(line.from, line.from, Decoration.line({ class: cls }));
    const code = copyTargets.get(n);
    if (code !== undefined) {
      // 挂在**首行末尾**（`side: 1`）：CSS 上绝对定位到该行右缘，不占行盒、不参与
      // CM6 的行高测量，故不会动到滚动定位（同 §3.4a 对 `.cm-line` 纵向盒的纪律）。
      builder.add(
        line.to,
        line.to,
        Decoration.widget({ widget: new CodeCopyWidget(code, copyButton), side: 1 }),
      );
    }
  }
  return builder.finish();
}

/** 代码块行底色扩展（Live Preview 与源码模式共用；阅读模式由 MarkdownContent 呈现）。 */
export function codeBlockLinesExtension(host: EditorHost): Extension {
  const copyButton = host.markdown.CodeCopyButton;
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;

      constructor(view: EditorView) {
        this.decorations = buildCodeBlockLineDecorations(view.state, view.visibleRanges, copyButton);
      }

      update(update: ViewUpdate) {
        if (
          update.docChanged ||
          update.viewportChanged ||
          syntaxTree(update.state) !== syntaxTree(update.startState)
        ) {
          this.decorations = buildCodeBlockLineDecorations(
            update.view.state,
            update.view.visibleRanges,
            copyButton,
          );
        }
      }
    },
    { decorations: (plugin) => plugin.decorations },
  );
}
