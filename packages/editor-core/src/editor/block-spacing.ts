// 块级纵向节奏（PRD-06-011/017，Typora / Obsidian 长文排版对标）：给每个**文档直属块**
// 的首行加行装饰，CSS 侧据此上一份块间距（上一行非空时才加，空行本身已是间距——见
// `blockLines` 的注释），标题行再加一份章节留白。
//
// **为什么是行装饰 + padding，而不是给 `.cm-line` 加 margin**：CM6 以 `.cm-line` 的
// 盒高做行高测量与滚动定位，纵向 margin 不计入该盒（相邻块之间还会合并），测量值会与
// 实际渲染错开，表现为滚动跳动与光标定位偏移。padding 计入盒高——代码块首末行的内缘
// 留白（`code-block-lines.ts` + `--md-block-inset-*`）用的就是同一机制，此处沿用。
// 物理 padding 属性归 `@lunarca/styles/codemirror.css` 单点负责（TDD-15 §3.3d），
// 本扩展只输出语义 class。
//
// **只认文档直属块**：列表项内的段落、引用块内的标题都不各领一份间距，否则一条
// 三项列表会被撑成三个独立块、一段带标题的标注块会被从中间掰开。判据是"父节点是
// Document"，不是正则——围栏代码块里的 `# 注释` 行不是标题（lezer 不会在 FencedCode
// 里解析出 ATXHeading）。
//
// **两个编辑模式共用**（与 `code-block-lines.ts` / `list-indent.ts` 同一裁决，而不同于
// 只在 Live Preview 生效的 `.luna-md-quoteline`）：块间距不是任何 Markdown 标记的**替身**
// ——它不隐藏也不改写一个字符，源码模式照样看得见完整 `#`、`>`、`-`。源码模式的定位是
// "看见原始 Markdown"，不是"放弃排版"；若只在 Live Preview 生效，三态里的 Live⇄源码
// 一切就会整篇纵向抖动一次，等于把刚修好的跳变搬了个家。

import { syntaxTree } from "@codemirror/language";
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

/** 参与纵向节奏的文档直属块（lezer-markdown 节点名，含 GFM 表格）。 */
const BLOCK_NODES = new Set([
  "Paragraph",
  "Blockquote",
  "BulletList",
  "OrderedList",
  "FencedCode",
  "CodeBlock",
  "HorizontalRule",
  "Table",
  "HTMLBlock",
  "CommentBlock",
  "LinkReference",
]);

const HEADING_NODE = /^(?:ATX|Setext)Heading[1-6]$/u;

function isBlockNode(name: string): boolean {
  return BLOCK_NODES.has(name) || HEADING_NODE.test(name);
}

/**
 * 块首行装饰的四种形态：普通块 / 标题 × 上一行空 / 上一行非空（`data-tight`）。
 *
 * **空行已经是间距**：Markdown 里块与块之间几乎总隔着一个空行，而空行在编辑面是一条
 * 实打实占一行高的 `.cm-line`。此前不看上一行、块首一律再加一份块间距，段落之间就是
 * "一行空白 + 16px"——比 Obsidian（一行）多出六成、比 Typora（约 1em）多出一倍多，长文
 * 读起来稀稀拉拉（真机 2026-09-04）。裁决：**块间距的目标值 = 一个正文行高**，上一行是
 * 空行时它已经兑现、不再加；上一行非空（列表紧接段落、标题紧接正文、围栏收口紧接正文）
 * 才由内距补上。标题在此之上另加一份章节留白（`--editor-heading-gap`），有没有空行都加。
 * 阅读面没有空行这回事，故每个块恒加一份——两面看到的间距因此相等（TDD-15 §3.4a）。
 */
const blockLines = {
  block: Decoration.line({ class: "luna-md-blockline" }),
  blockTight: Decoration.line({
    class: "luna-md-blockline",
    attributes: { "data-tight": "true" },
  }),
  heading: Decoration.line({
    class: "luna-md-blockline",
    attributes: { "data-block": "heading" },
  }),
  headingTight: Decoration.line({
    class: "luna-md-blockline",
    attributes: { "data-block": "heading", "data-tight": "true" },
  }),
} as const;

function blockLineDeco(heading: boolean, tight: boolean): Decoration {
  if (heading) {
    return tight ? blockLines.headingTight : blockLines.heading;
  }
  return tight ? blockLines.blockTight : blockLines.block;
}

/** 构建块间距行装饰（state 级纯函数，可无头测试）。 */
export function buildBlockSpacingDecorations(
  state: EditorState,
  ranges: readonly Range[],
): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  const seen = new Set<number>();
  for (
    let node = syntaxTree(state).topNode.firstChild;
    node;
    node = node.nextSibling
  ) {
    if (!isBlockNode(node.name)) {
      continue;
    }
    if (!ranges.some((range) => node.from <= range.to && node.to >= range.from)) {
      continue;
    }
    const line = state.doc.lineAt(node.from);
    // 文首不需要"与上一块拉开"——那里没有上一块，只会在正文顶端凭空多出一截空白。
    if (line.number === 1 || seen.has(line.number)) {
      continue;
    }
    seen.add(line.number);
    const tight = state.doc.line(line.number - 1).text.trim() !== "";
    builder.add(line.from, line.from, blockLineDeco(HEADING_NODE.test(node.name), tight));
  }
  return builder.finish();
}

/** 块间距扩展（Live Preview 与源码模式共用；阅读模式由 `.luna-reading` 的 CSS 承担）。 */
export function blockSpacingExtension(): Extension {
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;

      constructor(view: EditorView) {
        this.decorations = buildBlockSpacingDecorations(view.state, view.visibleRanges);
      }

      update(update: ViewUpdate) {
        if (
          update.docChanged ||
          update.viewportChanged ||
          syntaxTree(update.state) !== syntaxTree(update.startState)
        ) {
          this.decorations = buildBlockSpacingDecorations(
            update.view.state,
            update.view.visibleRanges,
          );
        }
      }
    },
    { decorations: (plugin) => plugin.decorations },
  );
}
