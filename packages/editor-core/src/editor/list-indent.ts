// 列表行悬挂缩进（PRD-06-011，Obsidian 对标）：软折行后的续行对齐到**内容起点**，
// 而不是顶回行首——否则圆点右侧的第二行比第一行还靠左，列表读起来是散的；嵌套
// 层级也随前导空格自然递进。
//
// 做法：给每个 ListItem 的**标记行**加行装饰，注入 `--md-hang`（标记前缀的渲染宽度），
// CSS 侧 `padding-inline-start: var(--md-hang)` + 等量负 `text-indent`——首行退回原位、
// 续行落在内容起点。被换成圆点 widget 的无序标记不用 `ch`，用设计 Token
// `--editor-list-indent`——它同时是阅读面的层级步长（两面同源），于是这一段是**精确**的。
//
// 原样可见的前缀（前导空格 / 有序编号 / 任务框）仍按 `ch` 估：比例字体里空格明显窄于
// 数字，故多层缩进的列表项，其软折行续行会比首行内容起点偏出十来个像素——这是已知的
// 近似，不是新账。要做准只有两条路：逐行测像素（每次视口更新为每行跑一次布局），
// 或把前导缩进也换成定宽 widget（等于让缩进区不可落光标，伤 Tab/退格调层级）。两条
// 都比它修的问题贵，故留在这里明说，不在注释里假装它是"一两个像素"。
//
// **必须走语法树找 ListItem，不能对每行跑正则**：围栏代码块里的 `- foo` 是代码
// 不是列表，正则会把它也缩进；lezer 不会在 FencedCode 里解析出 ListItem。

import { syntaxTree } from "@codemirror/language";
import { RangeSetBuilder, type EditorState, type Extension } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  ViewPlugin,
  type ViewUpdate,
  EditorView,
} from "@codemirror/view";

interface Range {
  from: number;
  to: number;
}

/** 一条列表标记行的悬挂宽度，拆成两种量纲（见 [`listHangWidth`]）。 */
export interface ListHang {
  /** 原样可见的前缀字符数（前导空格 + 有序编号 + 任务框），按 `ch` 计。 */
  chars: number;
  /** 被换成定宽圆点 widget 的无序标记个数，每个占一格 `--editor-list-indent`。 */
  bullets: number;
  /**
   * 源码模式下整段前缀的**字符**长度（前导空格 + 全部标记连同其后空格），按 `ch` 计。
   *
   * 悬挂宽度是**渲染**宽度，而同一行在两个模式里渲染出两种前缀：Live 把无序标记
   * 换成 `--editor-list-indent` 定宽 widget，源码模式原样显示 `* ` 裸字符（≈2ch）。
   * 只按 widget 计的公式在源码模式会多悬挂 `bullets × (indent − 2ch)`——六个标记的
   * 畸形行续行比首行内容起点深出约一倍（2026-08-23 真机截图）。
   */
  sourceChars: number;
}

/** 逐个吃掉一条列表行前缀里的标记：`([-*+] | 1.)` + 可选任务框 + 其后空格。 */
const MARKER_RE = /^([-*+]|\d{1,9}[.)])(\s+\[[ xX]\])?(\s+)/;

/**
 * 一条列表标记行的悬挂宽度——**渲染**宽度，不是源码字符数。
 *
 * **一行可以有不止一个标记**：`* * 文字` 在 CommonMark 里是「一个只含一个列表的
 * 列表」，两个标记都会被 Live Preview 各换成一个定宽圆点。此前只认第一个（正则一次性
 * 匹配、`buildListIndentDecorations` 又按行去重），于是 AI 生成的 `* * …` / `* * * * * * …`
 * 会算出一格悬挂却渲染出 N 格前缀：最后一个圆点与正文之间空出 N-1 格，软折行的续行
 * 还比首行浅 N-1 格。
 *
 * 无序标记连同其后整段空格都归 widget，故与空格打几个无关（`*   文字` 与 `- 文字`
 * 同宽）；有序/任务项标记原样可见，按源码长度计。
 * `- a` → 0ch+1；`  - a` → 2ch+1；`* * a` → 0ch+2；`12. a` → 4ch+0；`- [ ] a` → 4ch+1。
 * 非列表行 → null。
 */
export function listHangWidth(lineText: string): ListHang | null {
  const leading = /^\s*/.exec(lineText)?.[0].length ?? 0;
  let rest = lineText.slice(leading);
  const hang: ListHang = { chars: leading, bullets: 0, sourceChars: leading };
  for (;;) {
    const match = MARKER_RE.exec(rest);
    if (!match) {
      break;
    }
    if (/^[-*+]$/.test(match[1]) && match[2] === undefined) {
      hang.bullets += 1;
    } else {
      // 有序编号与任务框原样留在正文里，按它们真正占的字符算。
      hang.chars += match[0].length;
    }
    hang.sourceChars += match[0].length;
    rest = rest.slice(match[0].length);
    if (match[2] !== undefined) {
      break; // 任务框之后是正文，不会再有嵌套标记。
    }
  }
  return hang.bullets === 0 && hang.chars === leading ? null : hang;
}

/**
 * 悬挂宽度 → 行内注入的两个模式变量。两种量纲相加而不是折算成同一种：`ch` 是源码
 * 字符的近似宽度，圆点是设计 Token 定的定宽格——把后者也写成 `ch` 正是旧实现对不齐
 * 的第二个来源（widget 的 `2ch` 量在**粗体**字面上，与行内的 `ch` 差约 2px/个）。
 *
 * 装饰对两个编辑模式共用（同 `block-spacing`），但两个模式的前缀渲染宽度不同（见
 * `ListHang.sourceChars`），故此处只出**两份事实**，选用哪份由 CSS 按
 * `.luna-editor[data-view-mode]` 决定——模式切换零重建装饰。
 */
export function listHangStyle(hang: ListHang): string {
  const live = `calc(${hang.chars}ch + ${hang.bullets} * var(--editor-list-indent))`;
  return `--md-hang-live: ${live}; --md-hang-source: ${hang.sourceChars}ch`;
}

/** 构建列表悬挂缩进装饰（state 级纯函数，可无头测试）。 */
export function buildListIndentDecorations(
  state: EditorState,
  ranges: readonly Range[],
): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  const seen = new Set<number>();
  for (const { from, to } of ranges) {
    syntaxTree(state).iterate({
      from,
      to,
      enter(node) {
        if (node.name !== "ListItem") {
          return;
        }
        const line = state.doc.lineAt(node.from);
        if (seen.has(line.number)) {
          return;
        }
        seen.add(line.number);
        const hang = listHangWidth(line.text);
        if (hang === null) {
          return;
        }
        builder.add(
          line.from,
          line.from,
          Decoration.line({
            class: "luna-md-listline",
            attributes: { style: listHangStyle(hang) },
          }),
        );
      },
    });
  }
  return builder.finish();
}

/** 列表悬挂缩进扩展（Live 与源码两模式共用；两模式前缀宽度不同，见 `listHangStyle`）。 */
export function listIndentExtension(): Extension {
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;

      constructor(view: EditorView) {
        this.decorations = buildListIndentDecorations(view.state, view.visibleRanges);
      }

      update(update: ViewUpdate) {
        if (update.docChanged || update.viewportChanged) {
          this.decorations = buildListIndentDecorations(update.view.state, update.view.visibleRanges);
        }
      }
    },
    { decorations: (plugin) => plugin.decorations },
  );
}
