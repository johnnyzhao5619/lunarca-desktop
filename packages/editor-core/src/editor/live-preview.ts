// Live Preview：光标之外隐藏 Markdown 语法标记（Obsidian 所见即所得对标，
// PRD-06-011）。事实源始终是 md 文本——本扩展只做**可见性装饰**：
// - 块级标记（`#`、`>`、`---` 分隔线）：光标/选区触及**所在行**即还原源码；
// - 无序列表的 `- ` **常驻圆点、永不还原**（裸字符只在源码模式看，理由见 ListMark 分支）；
// - 行内标记（`**`、`*`、`~~`、`` ` ``、链接 URL、`[[ ]]` 括号）：触及**元素范围**即还原；
// - 跨行的非空选区**一律不还原**（`revealsSyntax`），鼠标拖选期间装饰冻结（`dragSelectingField`）。
// - 围栏代码块：未触及围栏行时隐藏开头 ```/~~~ 与结束围栏，保留语言 Info String 作为
//   代码标题；触及围栏行才还原完整 Markdown，代码正文始终是原生可编辑文本。
// 渲染细节（引用边条、分隔线、圆点）由 `@lunarca/styles/editor.css` 按设计 Token 定义。

import { syntaxTree } from "@codemirror/language";
import type { SyntaxNode } from "@lezer/common";
import {
  type EditorState,
  type Extension,
  RangeSet,
  StateEffect,
  StateField,
} from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  ViewPlugin,
  type ViewUpdate,
  WidgetType,
} from "@codemirror/view";
import { isWikiEmbedAt } from "./wiki-link";
import { WIKI_LINK_RE } from "../reading/wiki-link-syntax";
import { isUnresolvedReferenceLink } from "./reference-links";
import { isCodeContext } from "./markdown-decorations";
import { ImageEmbedWidget, isLocalAssetSrc, type ImageEmbedLabels } from "./image-embed";
import type { EditorHost } from "./editor-host";
import { parseCalloutMarker, quotePrefixLength, type CalloutTone } from "../markdown/callout";
import { imageTargetOf } from "../markdown/markdown";

interface Range {
  from: number;
  to: number;
}

/** 图片就地渲染上下文（PRD-06-013）：地址交哪个宿主解析、相对图片的锚点是哪一篇。 */
export interface LivePreviewImageContext {
  host: EditorHost;
  notePath: string | null;
  /** 看大图 / 缩放把手的文案；不给则不挂这两个控件。 */
  labels?: ImageEmbedLabels;
}

/**
 * 光标点（空选区）落在 [from, to] 内，含端点：光标停在元素边缘（`**粗体**|`）就该还原
 * 标记——那是「进去改一下」的最短路径。块级标记、表格、图片**只认它**：非空选区是在
 * "选东西"，不是在改这一行的语法。
 *
 * 此前非空选区也按几何相触还原，于是三击整行、拖选一整项都会把 `**` / `#` 显露出来，
 * 正文当场位移两个字符（真机 2026-09-04 两轮都指出了这一点）。
 */
export function caretTouches(state: EditorState, from: number, to: number): boolean {
  return state.selection.ranges.some(
    (range) => range.empty && range.from >= from && range.from <= to,
  );
}

/**
 * 某个非空选区的端点落在 (from, to) **内部**——选区从元素中间开始或在中间结束。行内标记
 * 只在这时才为非空选区还原：端点落在被藏起的 `**` 里时，不还原就看不见选区到底停在哪。
 * 整个包住元素的选区（三击整行、选中整个粗体词）不还原——那是复制 / 格式化的意图。
 */
export function boundaryInside(state: EditorState, from: number, to: number): boolean {
  return state.selection.ranges.some(
    (range) =>
      !range.empty &&
      ((range.from > from && range.from < to) || (range.to > from && range.to < to)),
  );
}

/** 选区形态：光标点 / 单行内的非空选区 / 跨行的非空选区。 */
export type SelectionShape = "caret" | "inline" | "block";

/**
 * 多选区取最"重"的形态。尾端恰在某行行首 = 只含上一行的换行符，不算跨进那一行
 * （三击整行选中的正是这种选区）。
 */
export function selectionShape(state: EditorState): SelectionShape {
  let shape: SelectionShape = "caret";
  for (const range of state.selection.ranges) {
    if (range.empty) {
      continue;
    }
    const first = state.doc.lineAt(range.from);
    const last = state.doc.lineAt(range.to);
    const endLine = last.from === range.to ? last.number - 1 : last.number;
    if (endLine > first.number) {
      return "block";
    }
    shape = "inline";
  }
  return shape;
}

/**
 * 跨行选区**不还原任何语法**（Typora / Notion / Bear 口径；Obsidian 会还原，也是它最常被
 * 抱怨的一点）：选多行的意图是复制、格式化或喂给 AI，不是编辑语法。还原只会让选区里的
 * 文字在指针下面横跳——`**` 一出现正文就位移两个字符，松手时选中的字和看到的不是一批。
 */
export function revealsSyntax(state: EditorState): boolean {
  return selectionShape(state) !== "block";
}

/** 块级语法（表格预览等）「该不该还原」的完整判定 = 选区形态允许 × 光标点落在其中。 */
export function syntaxTouches(state: EditorState, from: number, to: number): boolean {
  return revealsSyntax(state) && caretTouches(state, from, to);
}

interface TouchPredicates {
  /** 行内元素：光标点触及，或非空选区的端点落在元素内部。 */
  range: (from: number, to: number) => boolean;
  /** 块级标记所在行：只认光标点。 */
  line: (pos: number) => boolean;
}

/** 一次构建里形态只算一遍，逐节点只做几何比较。 */
function touchPredicates(state: EditorState): TouchPredicates {
  const reveal = revealsSyntax(state);
  return {
    range: (from, to) =>
      reveal && (caretTouches(state, from, to) || boundaryInside(state, from, to)),
    line(pos) {
      const line = state.doc.lineAt(pos);
      return reveal && caretTouches(state, line.from, line.to);
    },
  };
}

/**
 * 拖选进行中（鼠标左键按住）。期间**冻结**显隐装饰：每次选区变化都重建，指针下的文字
 * 就会随标记的时隐时现来回位移，松手时选中的和看到的不是同一批字。松手那一刻按最终
 * 选区重建一次。只管鼠标——键盘扩选一步一停，没有"指针下面"这回事。
 */
const setDragSelecting = StateEffect.define<boolean>();

export const dragSelectingField = StateField.define<boolean>({
  create() {
    return false;
  },
  update(value, tr) {
    for (const effect of tr.effects) {
      if (effect.is(setDragSelecting)) {
        return effect.value;
      }
    }
    return value;
  },
});

export function dragSelectionExtension(): Extension {
  return [
    dragSelectingField,
    EditorView.domEventHandlers({
      mousedown(event, view) {
        if (event.button !== 0) {
          return false;
        }
        view.dispatch({ effects: setDragSelecting.of(true) });
        // 松手可能发生在编辑器外（拖出正文、拖到气泡上），故听 window 而不是 contentDOM。
        const onUp = () => {
          window.removeEventListener("mouseup", onUp);
          if (view.dom.isConnected) {
            view.dispatch({ effects: setDragSelecting.of(false) });
          }
        };
        window.addEventListener("mouseup", onUp);
        return false;
      },
    }),
  ];
}

function isDragSelecting(state: EditorState): boolean {
  return state.field(dragSelectingField, false) ?? false;
}

const hideMark = Decoration.replace({});

/** 折叠成一段文字的 `[文本](url)`——点击处理器据它判"这一下点的是链接还是源码"。 */
const linkBodyMark = Decoration.mark({ class: "luna-md-linkbody" });

/**
 * 列表标记的隐藏（任务项的 `- `）——与 `hideMark` 同形但**另起一个实例**：它和圆点 widget
 * 一起构成原子区间（见 `listMarkAtoms`），按实例身份筛出来，不靠 spec 上的自定义字段
 * （`ReplaceDecorationSpec` 没有索引签名）。
 */
const hideListMark = Decoration.replace({});

/**
 * 无序项圆点（替换 `- ` / `* ` 标记连同其后空格）。
 *
 * 字形不写在这里：`data-depth` 只声明层级，形状与颜色由 `--editor-list-marker-*` 给，
 * 阅读面的 `::marker` 消费的是同一组 Token（两面同源，见该组 Token 的定义处）。
 * 层级封顶 3（同浏览器默认阶：第 3 层起不再变形状）。
 */
export const BULLET_DEPTH_MAX = 3;

export class BulletWidget extends WidgetType {
  constructor(readonly depth: number) {
    super();
  }

  eq(other: BulletWidget) {
    return other.depth === this.depth;
  }

  toDOM() {
    const el = document.createElement("span");
    el.className = "luna-md-bullet";
    el.dataset.depth = String(this.depth);
    return el;
  }
}

/**
 * 常驻替换的列表标记区间 → `EditorView.atomicRanges`：光标左右移动整段跳过、退格整段
 * 删除。没有这一条，`- ` 藏在圆点后面却仍能落光标——从正文首字向左一步，光标停在 `-`
 * 与空格之间，敲出的字符把列表拆成 `-x 正文`。
 */
export function listMarkAtoms(decorations: DecorationSet): RangeSet<Decoration> {
  return decorations.update({
    filter: (_from, _to, deco) => deco === hideListMark || deco.spec.widget instanceof BulletWidget,
  });
}

/** 节点所处的列表层级（1 = 最外层；封顶 `BULLET_DEPTH_MAX`）。 */
export function listDepthOf(node: SyntaxNode): number {
  let depth = 0;
  for (let parent = node.parent; parent; parent = parent.parent) {
    if (parent.name === "BulletList" || parent.name === "OrderedList") {
      depth += 1;
    }
  }
  return Math.min(Math.max(depth, 1), BULLET_DEPTH_MAX);
}

class HrWidget extends WidgetType {
  toDOM() {
    const el = document.createElement("span");
    el.className = "luna-md-hrline";
    return el;
  }
}

const quoteLine = Decoration.line({ class: "luna-md-quoteline" });

/**
 * 有序列表编号的着色（`1.` / `1)`）——与无序圆点 widget **同一组 Token**，两种列表
 * 在同一篇里视觉权重相等（TDD-15 §3.4a 列表落地点）。这里只加 class，编号字符原样
 * 留在文档里可编辑（不做 widget：那会让编号不可落光标，伤序号的手动改写）。
 */
const orderedListMark = Decoration.mark({ class: "luna-md-ordered-mark" });

/**
 * 标注块头部的类型标签（PRD-06-011）。替换 `[!type]±` 标记本身——**正文一个字都不动**，
 * 光标一落到该行标记就原样还原（与其它行级标记同一心智）。
 *
 * 标签文字取自文档里的类型词，不是界面文案，故不进 i18n。
 */
export class CalloutLabelWidget extends WidgetType {
  constructor(
    readonly label: string,
    readonly tone: CalloutTone,
  ) {
    super();
  }

  eq(other: CalloutLabelWidget) {
    return other.label === this.label && other.tone === this.tone;
  }

  toDOM() {
    const el = document.createElement("span");
    el.className = "luna-md-callout__label";
    el.dataset.callout = this.tone;
    el.textContent = this.label;
    return el;
  }
}

/** 标注块行在块内的位置（决定圆角与上下留白）。 */
type CalloutLinePos = "solo" | "head" | "body" | "tail";

function calloutLineDeco(tone: CalloutTone, pos: CalloutLinePos): Decoration {
  return Decoration.line({
    class: "luna-md-calloutline",
    attributes: { "data-callout": tone, "data-callout-pos": pos },
  });
}
const hiddenFenceLine = Decoration.line({ class: "luna-md-codeblock-line--fence-hidden" });

/** 行内标记节点名 → 需要触及的父元素节点名集合。 */
const INLINE_MARK_PARENTS: Record<string, readonly string[]> = {
  EmphasisMark: ["Emphasis", "StrongEmphasis"],
  StrikethroughMark: ["Strikethrough"],
  CodeMark: ["InlineCode"],
};

/**
 * 构建 Live Preview 装饰（state 级纯函数，可无头测试）。
 * 返回的装饰含：标记隐藏（replace 空）、圆点/分隔线 widget、引用行 class。
 */
export function buildLivePreviewDecorations(
  state: EditorState,
  ranges: readonly Range[],
  imageCtx?: LivePreviewImageContext,
): DecorationSet {
  const touched = touchPredicates(state);
  const decos: { from: number; to: number; deco: Decoration }[] = [];
  const quoteLines = new Set<number>();
  const calloutLines = new Map<number, { tone: CalloutTone; pos: CalloutLinePos }>();
  /**
   * 标注头 `[!type]±` 的字节区间。lezer 把它当成链接标签解析（`[` `]` 各产一个 LinkMark），
   * 那两个标记装饰会与整段替换**部分重叠**而在去重时把后者挤掉——标记区间内的其它隐藏
   * 装饰一律让位，它们描述的是同一段字符的另一种（错误的）读法。
   */
  const calloutMarkerSpans: { from: number; to: number }[] = [];
  /**
   * 有序列表编号（`1.`）的着色区间。**刻意不进 `decos`**：那个数组末尾有一道
   * "replace 部分重叠即丢弃"的防御，它按 `from < to` 判定，会把这些非替换的 mark
   * 一并当成隐藏区间参与去重。mark 与 replace 重叠在 CM6 里本就合法，故单独收集、
   * 最后并入结果集。
   */
  const orderedMarks: { from: number; to: number }[] = [];

  const hideFenceLine = (lineNumber: number, from: number, to: number) => {
    const line = state.doc.line(lineNumber);
    decos.push({ from, to, deco: hideMark });
    decos.push({ from: line.from, to: line.from, deco: hiddenFenceLine });
  };

  const hideWithFollowingSpace = (from: number, to: number) => {
    const next = state.doc.sliceString(to, to + 1);
    decos.push({ from, to: next === " " ? to + 1 : to, deco: hideMark });
  };

  for (const range of ranges) {
    syntaxTree(state).iterate({
      from: range.from,
      to: range.to,
      enter(node) {
        switch (node.name) {
          case "FencedCode":
          case "CodeBlock": {
            const opening = state.doc.lineAt(node.from);
            // CommonMark 围栏：最多三个空格缩进 + 至少三个同类反引号/波浪线；
            // Info String 保持原文可见，作为未聚焦时的语言标题。
            const openingMatch = /^(\s*)(`{3,}|~{3,})/u.exec(opening.text);
            if (!openingMatch) {
              return;
            }
            const markerFrom = opening.from + openingMatch[1].length;
            const markerTo = markerFrom + openingMatch[2].length;
            if (!touched.line(opening.from)) {
              hideFenceLine(opening.number, markerFrom, markerTo);
            }

            // 结束围栏没有用户可读的信息；未触及时整行隐藏，仅留下容器的底部留白。
            // 围栏尚未闭合的输入中，最后一行是代码正文，不能误隐藏。
            const closing = state.doc.lineAt(node.to);
            const closingMatch = /^(\s*)(`+|~+)\s*$/u.exec(closing.text);
            if (
              closing.number !== opening.number &&
              closingMatch !== null &&
              closingMatch[2][0] === openingMatch[2][0] &&
              closingMatch[2].length >= openingMatch[2].length &&
              !touched.line(closing.from)
            ) {
              hideFenceLine(closing.number, closing.from, closing.to);
            }
            return;
          }
          case "HeaderMark": {
            // ATX `#`（Setext `===` 同样收口为标记行为：行触及即还原）。
            if (!touched.line(node.from)) {
              hideWithFollowingSpace(node.from, node.to);
            }
            return;
          }
          case "QuoteMark": {
            if (!touched.line(node.from)) {
              hideWithFollowingSpace(node.from, node.to);
            }
            return;
          }
          case "Blockquote": {
            const firstLine = state.doc.lineAt(node.from);
            const lastLine = state.doc.lineAt(node.to);
            const prefix = quotePrefixLength(firstLine.text);
            const marker = parseCalloutMarker(firstLine.text.slice(prefix));
            if (!marker) {
              // 普通引用块：每行加左边条 class（与是否还原源码无关，Obsidian 同样常驻）。
              for (let n = firstLine.number; n <= lastLine.number; n++) {
                quoteLines.add(n);
              }
              return;
            }
            // 标注块（PRD-06-011）：整块换成带色调的容器，`[!type]±` 收成类型标签。
            for (let n = firstLine.number; n <= lastLine.number; n++) {
              const pos: CalloutLinePos =
                firstLine.number === lastLine.number
                  ? "solo"
                  : n === firstLine.number
                    ? "head"
                    : n === lastLine.number
                      ? "tail"
                      : "body";
              calloutLines.set(n, { tone: marker.tone, pos });
            }
            if (!touched.line(firstLine.from)) {
              const markerFrom = firstLine.from + prefix;
              const markerTo = markerFrom + marker.markerEnd;
              calloutMarkerSpans.push({ from: markerFrom, to: markerTo });
              decos.push({
                from: markerFrom,
                to: markerTo,
                deco: Decoration.replace({
                  widget: new CalloutLabelWidget(marker.label, marker.tone),
                }),
              });
            }
            return;
          }
          case "ListMark": {
            // 仅无序圆点替换；有序编号**保留字符**（Obsidian 同），只着色。任务项的
            // `-` 直接隐藏（复选框 widget 已承担视觉锚点）。
            const text = state.sliceDoc(node.from, node.to);
            if (!/^[-*+]$/u.test(text)) {
              // 有序编号（`1.` / `1)`）：加一层着色 mark，让它与无序圆点同权重。
              // 不做替换、不隐藏任何字符，故**与光标在不在这一行无关**——没有源码
              // 可还原，加行触及判断只会让编号随光标移动忽明忽暗。
              orderedMarks.push({ from: node.from, to: node.to });
              return;
            }
            // **圆点常驻，光标落上去也不还原**（Obsidian / Typora / Notion / Bear 一致；
            // 源码模式本就不装本扩展，裸 `-` 在那里看）。曾经按行触及还原：裸 `- ` 比
            // 一格定宽圆点窄十几像素，而悬挂缩进按圆点算——被触及行的首行正文左移、
            // 续行不动，两行错开（真机 2026-09-04）。替换区间登记为原子区间：光标
            // 不落进 `- ` 内部，项首退格一次删掉整个标记（等于 Obsidian 的"退格去圆点"）。
            const isTask = /^ \[[ xX]\]/u.test(
              state.doc.sliceString(node.to, node.to + 4),
            );
            if (isTask) {
              const next = state.doc.sliceString(node.to, node.to + 1);
              decos.push({ from: node.from, to: next === " " ? node.to + 1 : node.to, deco: hideListMark });
            } else {
              // 连同其后的**整段空格**一起替换：圆点 widget 定宽 = 一格层级步长
              // （`--editor-list-indent`），首行内容起点与续行像素级对齐——源码空格比
              // 一格窄，留着它首行会比续行浅半个字符；`*   文字` 这类多空格写法同理。
              // 悬挂契约见 list-indent.ts：**每个**被换掉的无序标记各计一格步长——
              // `* * 文字`（合法的一行内嵌套）会在同一行产出两个 widget。
              const tail = /^ +/u.exec(state.doc.sliceString(node.to, node.to + 8));
              decos.push({
                from: node.from,
                to: node.to + (tail?.[0].length ?? 0),
                deco: Decoration.replace({ widget: new BulletWidget(listDepthOf(node.node)) }),
              });
            }
            return;
          }
          case "HorizontalRule": {
            if (!touched.line(node.from)) {
              decos.push({
                from: node.from,
                to: node.to,
                deco: Decoration.replace({ widget: new HrWidget() }),
              });
            }
            return;
          }
          case "EmphasisMark":
          case "StrikethroughMark":
          case "CodeMark": {
            const parents = INLINE_MARK_PARENTS[node.name];
            const parent = node.node.parent;
            if (!parent || !parents.includes(parent.name)) {
              return; // fenced code 的 CodeMark 等：保留源码。
            }
            if (!touched.range(parent.from, parent.to)) {
              decos.push({ from: node.from, to: node.to, deco: hideMark });
            }
            return;
          }
          case "Image": {
            // `![alt](rel)`：**光标不在图片所在行**时替换为就地渲染的图片（PRD-06-011/013，
            // Obsidian Live Preview 对标）；光标落在该行则还原源码可编辑链接。远程/非本地图片
            // 保持源码（离线纪律）。
            if (!imageCtx || touched.line(node.from)) {
              return;
            }
            // 判定走**语法树**、且与阅读面同一个函数（TDD-15 §3.4a）：此前这里是一条
            // 手写正则，`"标题"` / `<带空格路径>` / 转义括号 / alt 里带 `]` 一律漏判或
            // 解析出错误的 src。语法树本来就在手上（`node` 就是 lezer 的 `Image`）。
            const info = imageTargetOf(node.node, (from, to) => state.sliceDoc(from, to));
            if (!info || !isLocalAssetSrc(info.src)) {
              return;
            }
            decos.push({
              from: node.from,
              to: node.to,
              deco: Decoration.replace({
                widget: new ImageEmbedWidget(
                  imageCtx.host,
                  imageCtx.notePath,
                  info.src,
                  info.alt,
                  info.width,
                  imageCtx.labels,
                ),
              }),
            });
            return;
          }
          case "Link": {
            // `[文本](url)`：未触及时隐藏 `[`、`](url)`，仅展示文本。
            if (touched.range(node.from, node.to)) {
              return;
            }
            // 没有 `URL` 子节点 = 引用式链接（`[1]` / `[1][]` / `[1][4]`），它是不是
            // 一条真链接**取决于文档里有没有对应的 `[label]: url` 定义**，而语法树两种
            // 情况给的是同一棵树。解析不到定义就不折叠——照原样显示 `[1]`，让"这不是
            // 链接"一眼可见；去链接色由 `reference-links.ts` 负责（两模式共用同一判据）。
            if (isUnresolvedReferenceLink(state, node.node)) {
              return;
            }
            const cursor = node.node.cursor();
            if (!cursor.firstChild()) {
              return;
            }
            const marks: Range[] = [];
            do {
              if (
                cursor.name === "LinkMark" ||
                cursor.name === "URL" ||
                cursor.name === "LinkTitle"
              ) {
                marks.push({ from: cursor.from, to: cursor.to });
              }
            } while (cursor.nextSibling());

            for (const mark of marks) {
              decos.push({ from: mark.from, to: mark.to, deco: hideMark });
            }
            // 折叠态**独有**的一个类名，给点击处理器当抓手（与 wiki-link 同一套路）：
            // 语法高亮那份 `.luna-md-link` 在源码模式下同样在场，按它判就会让"点一下想
            // 改 URL"变成"跳走了"。而这一条只在链接被折叠成一段文字时才挂上。
            //
            // 只盖**留在屏幕上的那段文字**（第一个 `[` 之后到 `]` 之前），不是整条
            // 链接：本函数末尾那道去重按"后来的落在前一条里就丢掉"收敛，盖住整条会把
            // 里面所有折叠标记连锅端掉——链接于是根本折不起来。
            const open = marks[0];
            const close = marks[1];
            if (open && close && open.to < close.from) {
              decos.push({ from: open.to, to: close.from, deco: linkBodyMark });
            }
            return;
          }
          default:
        }
      },
    });

    // wiki-link 括号折叠：`[[stem]]` → stem；`[[stem|alias]]` → alias。
    let pos = range.from;
    while (pos <= range.to) {
      const line = state.doc.lineAt(pos);
      WIKI_LINK_RE.lastIndex = 0;
      let m: RegExpExecArray | null;
      while ((m = WIKI_LINK_RE.exec(line.text)) !== null) {
        const start = line.from + m.index;
        const end = start + m[0].length;
        if (touched.range(start, end) || isCodeContext(state, start + 1) || isWikiEmbedAt(line.text, m.index)) {
          continue;
        }
        if (m[2] !== undefined) {
          // [[stem|alias]]：隐藏 `[[stem|` 与 `]]`。
          decos.push({
            from: start,
            to: start + 2 + m[1].length + 1,
            deco: hideMark,
          });
        } else {
          decos.push({ from: start, to: start + 2, deco: hideMark });
        }
        decos.push({ from: end - 2, to: end, deco: hideMark });
      }
      pos = line.to + 1;
    }
  }

  for (const lineNumber of quoteLines) {
    // 标注块内的嵌套引用不再叠一条引用左边条（标注容器已经是这块的视觉边界）。
    if (calloutLines.has(lineNumber)) {
      continue;
    }
    const line = state.doc.line(lineNumber);
    decos.push({ from: line.from, to: line.from, deco: quoteLine });
  }

  for (const [lineNumber, { tone, pos }] of calloutLines) {
    const line = state.doc.line(lineNumber);
    decos.push({ from: line.from, to: line.from, deco: calloutLineDeco(tone, pos) });
  }

  // 防御：replace 装饰部分重叠非法（如极端嵌套语法双重命中），按起点排序后丢弃
  // 与已接受隐藏范围重叠的后来者；零宽 line 装饰不受影响。
  decos.sort((a, b) => a.from - b.from || a.to - b.to);
  const accepted: typeof decos = [];
  let lastHideEnd = -1;
  for (const d of decos) {
    if (
      d.from < d.to &&
      calloutMarkerSpans.some(
        (span) =>
          d.from >= span.from &&
          d.to <= span.to &&
          !(d.from === span.from && d.to === span.to),
      )
    ) {
      continue;
    }
    if (d.from < d.to) {
      if (d.from < lastHideEnd) {
        continue;
      }
      lastHideEnd = d.to;
    }
    accepted.push(d);
  }

  return Decoration.set(
    [
      ...accepted.map((d) => d.deco.range(d.from, d.to)),
      ...orderedMarks.map((m) => orderedListMark.range(m.from, m.to)),
    ],
    true,
  );
}

/** Live Preview 扩展：文档/视口/选区变化时重建装饰（拖选期间冻结，松手重建一次）。
 * `getNotePath` 供图片就地渲染解析相对路径（每次重建读取当前笔记路径）；图片地址与附件变化归 `host`。
 *
 * **输入法组字期间同样冻结**：组字时每个拼音字母都是一次文档变化，若每次都按新文本重建，
 * 组字区附近的语法标记会随之隐现、替换部件进进出出，WebKit 的组字 DOM 被改动后候选框跳位、
 * 已上屏的字被吞或重复（Obsidian 中文输入法问题的同一根因）。组字期间只把旧装饰随改动平移，
 * 组字结束后按最终文本重建一次。 */
export function livePreviewExtension(
  host: EditorHost,
  getNotePath?: () => string | null,
  imageLabels?: ImageEmbedLabels,
): Extension {
  const imageCtx = (): LivePreviewImageContext | undefined =>
    getNotePath ? { host, notePath: getNotePath(), labels: imageLabels } : undefined;
  const plugin = ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      atoms: RangeSet<Decoration>;
      /** 组字期间积下的「该重建而没重建」。 */
      stale = false;

      constructor(readonly view: EditorView) {
        this.decorations = buildLivePreviewDecorations(view.state, view.visibleRanges, imageCtx());
        this.atoms = listMarkAtoms(this.decorations);
        // 库外删掉 / 放回附件（PRD-06-012 ③）：图片部件带着新的附件代际号重建，重新判断指不指得到。
        this.stopAssetWatch = host.assets.subscribe(() => {
          this.stale = true;
          if (this.view.dom.isConnected) this.view.dispatch({});
        });
      }

      private readonly stopAssetWatch: () => void;

      destroy() {
        this.stopAssetWatch();
      }

      update(update: ViewUpdate) {
        if (update.view.composing) {
          if (update.docChanged) {
            this.decorations = this.decorations.map(update.changes);
            this.atoms = listMarkAtoms(this.decorations);
          }
          this.stale ||= update.docChanged || update.viewportChanged || update.selectionSet;
          return;
        }
        const dragging = isDragSelecting(update.state);
        const dragEnded = isDragSelecting(update.startState) && !dragging;
        if (
          this.stale ||
          update.docChanged ||
          update.viewportChanged ||
          dragEnded ||
          (update.selectionSet && !dragging)
        ) {
          this.stale = false;
          this.decorations = buildLivePreviewDecorations(
            update.view.state,
            update.view.visibleRanges,
            imageCtx(),
          );
          this.atoms = listMarkAtoms(this.decorations);
        }
      }

      /** 组字结束：CM 先把最后一笔落进文档，下一拍再补一次空事务触发重建。 */
      scheduleRebuild() {
        setTimeout(() => {
          if (this.stale && !this.view.composing && this.view.dom.isConnected) {
            this.view.dispatch({});
          }
        }, 0);
      }
    },
    {
      decorations: (instance) => instance.decorations,
      provide: (instance) =>
        EditorView.atomicRanges.of((view) => view.plugin(instance)?.atoms ?? RangeSet.empty),
      eventHandlers: {
        compositionend() {
          this.scheduleRebuild();
        },
      },
    },
  );
  return [dragSelectionExtension(), plugin];
}
