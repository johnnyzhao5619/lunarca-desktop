// 引用式链接的「解析得了 / 解析不了」判定与去链接化（PRD-06-011 / PRD-16-005）。
//
// **为什么它不能只在 Live Preview 里生效**：CommonMark 里 `[1]` 是引用式链接，lezer
// 无论文档里有没有对应的 `[1]: url` 定义都产出同一个 `Link` 节点（已实测）。于是一段普通
// 文字里的 `[1]`、`[3][5]`、甚至 `[Eric Mcelroy]` 都被 `tags.link` 着上强调色——用户看到
// 一个链接色的名字，点下去什么也不会发生。这正是 PRD-16-005 点名的那件事：「把角标做得
// 容易显示、难以点开就是在制造过度信任」。判定与源码里写了什么有关、与**用哪个视图看**
// 无关，故本扩展与 list-indent / block-spacing 同为 Live 与源码两模式共用；此前它长在
// Live Preview 的装饰构建里，同一句话在源码模式着色、在 Live 模式不着色。

import { syntaxTree } from "@codemirror/language";
import type { SyntaxNode } from "@lezer/common";
import { RangeSetBuilder, type EditorState, type Extension } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  ViewPlugin,
  type ViewUpdate,
} from "@codemirror/view";

interface Range {
  from: number;
  to: number;
}

/** 判为「解析不了」时挂的类：把 `tags.link` 的强调色收回来。 */
const unresolvedLinkMark = Decoration.mark({ class: "luna-md-link--unresolved" });

/** 本文档里出现过的链接引用定义标签（`[label]: url` 的 label，小写归一）。 */
const definedLabelsCache = new WeakMap<object, Set<string>>();

/**
 * 收集文档里的链接引用定义标签。
 *
 * 按 `state.doc` 身份缓存：`Text` 不可变，同一份文档反复求值必然同解，而本函数是
 * 全文扫描——不缓存的话每次视口更新都要为整篇笔记付一次。
 */
function definedLinkLabels(state: EditorState): Set<string> {
  const cached = definedLabelsCache.get(state.doc);
  if (cached) {
    return cached;
  }
  const labels = new Set<string>();
  syntaxTree(state).iterate({
    enter: (node) => {
      if (node.name !== "LinkReference") {
        return;
      }
      const label = node.node.getChild("LinkLabel");
      if (label) {
        labels.add(state.doc.sliceString(label.from + 1, label.to - 1).trim().toLowerCase());
      }
    },
  });
  definedLabelsCache.set(state.doc, labels);
  return labels;
}

/**
 * 这个 `Link` 节点是不是一条**解析不了**的引用式链接（`[1]` / `[1][]` / `[1][4]`
 * 而文档里没有对应的 `[label]: url`）。有 `URL` 子节点的行内链接恒为 false。
 *
 * Live Preview 用它决定「不折叠方括号」，本模块用它决定「不着链接色」——同一个判据，
 * 两个消费面共用一处实现。
 */
export function isUnresolvedReferenceLink(state: EditorState, node: SyntaxNode): boolean {
  let hasUrl = false;
  let labelNode: Range | null = null;
  const cursor = node.cursor();
  if (!cursor.firstChild()) {
    return false;
  }
  do {
    if (cursor.name === "URL") {
      hasUrl = true;
    }
    if (cursor.name === "LinkLabel") {
      labelNode = { from: cursor.from, to: cursor.to };
    }
  } while (cursor.nextSibling());
  if (hasUrl) {
    return false;
  }
  const label = labelNode
    ? state.doc.sliceString(labelNode.from + 1, labelNode.to - 1)
    : // 简写式 `[1]`：标签就是方括号里的文本本身。
      state.doc.sliceString(node.from + 1, node.to - 1);
  const key = label.trim().toLowerCase();
  // 折叠式 `[1][]` 的 `LinkLabel` 是空的，标签回落到前一对方括号。
  const effective = key === "" ? state.doc.sliceString(node.from + 1, node.to).split("]")[0] : key;
  return !definedLinkLabels(state).has(effective.trim().toLowerCase());
}

/** 构建去链接化装饰（state 级纯函数，可无头测试）。 */
export function buildUnresolvedLinkDecorations(
  state: EditorState,
  ranges: readonly Range[],
): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  for (const { from, to } of ranges) {
    syntaxTree(state).iterate({
      from,
      to,
      enter(node) {
        if (node.name === "Link" && isUnresolvedReferenceLink(state, node.node)) {
          builder.add(node.from, node.to, unresolvedLinkMark);
        }
      },
    });
  }
  return builder.finish();
}

/** 未解析引用式链接的去链接化扩展（Live 与源码两模式共用）。 */
export function unresolvedLinkExtension(): Extension {
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;

      constructor(view: EditorView) {
        this.decorations = buildUnresolvedLinkDecorations(view.state, view.visibleRanges);
      }

      update(update: ViewUpdate) {
        if (update.docChanged || update.viewportChanged) {
          this.decorations = buildUnresolvedLinkDecorations(
            update.view.state,
            update.view.visibleRanges,
          );
        }
      }
    },
    { decorations: (plugin) => plugin.decorations },
  );
}
