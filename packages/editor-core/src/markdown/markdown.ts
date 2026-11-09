/**
 * 共享 Markdown 解析（纯逻辑，零 React）：文本 → 结构化 MdNode 树。
 * 消费方：对话消息渲染（`ChatMarkdown`）与笔记阅读模式（`ReadingView`），
 * 渲染统一走 `render/MarkdownContent`（DRY，单一解析基线）。
 *
 * 解析器复用编辑器同一方言 `lunaMarkdownParser`（GFM 基线 + CJK 友好强调，`markdown-dialect.ts`；
 * ADR-006 生态内），渲染把节点树映射为
 * React 元素——**全程无 innerHTML**，天然免 XSS。lezer 解析容错，流式半成品文本可随
 * token 增量安全重解析。
 *
 * 未覆盖的块类型一律回退为「原文段落」（绝不丢内容）。
 */

import { lunaMarkdownParser, mathSource, splitEmbed } from "./markdown-dialect";
import type { SyntaxNode } from "@lezer/common";
import { parseCalloutMarker, stripQuoteLevel, type CalloutTone } from "./callout";

export type MdInline =
  | { kind: "text"; text: string }
  | { kind: "strong"; children: MdInline[] }
  | { kind: "em"; children: MdInline[] }
  | { kind: "del"; children: MdInline[] }
  | { kind: "code"; text: string }
  | {
      kind: "link";
      href: string;
      children: MdInline[];
      /**
       * 同一 `href` 在全文中的第几次出现（0 起，文档顺序）。纯排版事实：阅读器把它连同
       * href 交给 Rust，由 Rust 判定这一处是不是某条声明的出处——边界与编号不在前端算。
       */
      occurrence?: number;
    }
  | { kind: "image"; src: string; alt: string; width: number | null }
  /** `<br>`：GFM 表格单元格里换行的唯一写法，也是唯一放行的 HTML 标签（其余 HTML 按原文）。 */
  | { kind: "break" }
  /** 行内公式 `$…$`（TeX 源码，不含 `$`）。 */
  | { kind: "math"; tex: string }
  /** 脚注引用 `[^id]`；`note` 是配对定义的纯文本（配不上为 null，解析完统一挂上）。 */
  | { kind: "footnote"; id: string; note: string | null }
  /** Obsidian 嵌入 `![[目标|显示]]`（对标方案 D5）；指向什么由 Rust 解析。 */
  | { kind: "embed"; target: string; display: string | null };

export interface MdListItem {
  /** 任务项状态；非任务项为 undefined。 */
  task?: "done" | "todo";
  blocks: MdBlock[];
}

/**
 * 列对齐（GFM 分隔行 `:---` / `:--:` / `---:`）。
 *
 * **两个渲染面共用这一个类型与这一个解析函数**（TDD-15 §3.4a）：编辑面的表格
 * Live Preview（`editor/table-model.ts`）按行文本切分表格、阅读面按 lezer
 * 树遍历，切法不同，但"这一列往哪边靠"必须是同一个判断——此前阅读面**根本没读**
 * 对齐，同一张表在编辑面右对齐、切到阅读面全左对齐。
 */
export type TableAlign = "left" | "center" | "right" | null;

export type MdBlock =
  | { kind: "heading"; level: 1 | 2 | 3 | 4 | 5 | 6; children: MdInline[] }
  | { kind: "paragraph"; children: MdInline[] }
  /** 整段只有一条嵌入：按块显示（笔记嵌入是一块内容，不能塞进段落里）。 */
  | { kind: "embed"; target: string; display: string | null }
  | { kind: "code_block"; lang: string; code: string }
  /** 成块公式 `$$…$$`（TeX 源码，不含 `$$`）。 */
  | { kind: "math"; tex: string }
  /** 脚注定义 `[^id]: …`。 */
  | { kind: "footnote"; id: string; children: MdInline[] }
  | { kind: "blockquote"; blocks: MdBlock[] }
  | {
      kind: "callout";
      tone: CalloutTone;
      /** 展示用类型标签（取自文档里的类型词，非 i18n）。 */
      label: string;
      title: MdInline[];
      defaultFolded: boolean;
      blocks: MdBlock[];
    }
  | { kind: "list"; ordered: boolean; items: MdListItem[] }
  | { kind: "table"; header: MdInline[][]; align: TableAlign[]; rows: MdInline[][][] }
  | { kind: "hr" };

const HEADING_LEVELS: Record<string, 1 | 2 | 3 | 4 | 5 | 6> = {
  ATXHeading1: 1,
  ATXHeading2: 2,
  ATXHeading3: 3,
  ATXHeading4: 4,
  ATXHeading5: 5,
  ATXHeading6: 6,
  SetextHeading1: 1,
  SetextHeading2: 2,
};

/** 行内标记节点：只占位不产出内容（内容在标记之间的文本空隙里）。 */
const INLINE_MARKS = new Set([
  "FootnoteMark",
  "FootnoteLabel",
  "EmphasisMark",
  "CodeMark",
  "StrikethroughMark",
  "LinkMark",
  "HeaderMark",
  "TaskMarker",
  "URL",
  "LinkTitle",
  "LinkLabel",
]);

export function parseChatMarkdown(text: string): MdBlock[] {
  const tree = lunaMarkdownParser.parse(text);
  const blocks = blocksOf(tree.topNode, text);
  numberLinkOccurrences(blocks);
  attachFootnoteNotes(blocks);
  return blocks;
}

/** 按文档顺序给每个链接标注「同一 href 的第几次出现」（见 `MdInline.link.occurrence`）。 */
/** 按文档顺序访问每个行内节点（含嵌套在粗体、链接里的），块的遍历只写这一份。 */
function forEachInline(blocks: MdBlock[], visit: (node: MdInline) => void): void {
  const inlines = (nodes: MdInline[]) => {
    for (const node of nodes) {
      visit(node);
      if (node.kind === "link" || node.kind === "strong" || node.kind === "em" || node.kind === "del") inlines(node.children);
    }
  };
  const walk = (items: MdBlock[]) => {
    for (const block of items) {
      switch (block.kind) {
        case "heading":
        case "paragraph":
        case "footnote":
          inlines(block.children);
          break;
        case "blockquote":
          walk(block.blocks);
          break;
        case "callout":
          inlines(block.title);
          walk(block.blocks);
          break;
        case "list":
          for (const item of block.items) walk(item.blocks);
          break;
        case "table":
          for (const cell of block.header) inlines(cell);
          for (const row of block.rows) for (const cell of row) inlines(cell);
          break;
        case "code_block":
        case "math":
        case "hr":
          break;
      }
    }
  };
  walk(blocks);
}

function numberLinkOccurrences(blocks: MdBlock[]): void {
  const seen = new Map<string, number>();
  forEachInline(blocks, (node) => {
    if (node.kind !== "link") return;
    const count = seen.get(node.href) ?? 0;
    node.occurrence = count;
    seen.set(node.href, count + 1);
  });
}

/** 脚注引用挂上定义的纯文本（悬停提示用）；配不上的引用 `note` 为 null。 */
function attachFootnoteNotes(blocks: MdBlock[]): void {
  const notes = new Map<string, string>();
  const collect = (items: MdBlock[]) => {
    for (const block of items) {
      if (block.kind === "footnote") notes.set(block.id, inlineToPlainText(block.children).trim());
      if (block.kind === "blockquote" || block.kind === "callout") collect(block.blocks);
      if (block.kind === "list") for (const item of block.items) collect(item.blocks);
    }
  };
  collect(blocks);
  forEachInline(blocks, (node) => {
    if (node.kind === "footnote") node.note = notes.get(node.id) ?? null;
  });
}

/**
 * 行内片段解析：单元格 / 单行文本 → 行内节点序列（表格 Live Preview 等消费，
 * 与整篇渲染同一解析基线）。片段解析为单一段落时返回其行内子节点，否则
 * 原文回退（表格单元格里出现块语法按字面呈现，不丢内容）。
 */
export function parseInlineMarkdown(text: string): MdInline[] {
  const blocks = parseChatMarkdown(text);
  const first = blocks[0];
  if (blocks.length === 1 && first?.kind === "paragraph") {
    return first.children;
  }
  return text.length > 0 ? [{ kind: "text", text }] : [];
}

/** 行尾块标记 ` ^id`（Obsidian 写法，`![[笔记#^id]]` 嵌入与跳转用）。 */
const BLOCK_ID_TAIL = /\s\^[A-Za-z0-9-]+\s*$/u;

/**
 * 去掉段落末尾的块标记（只影响显示，文件一字不动）：它是给链接认路用的锚点，不是内容——
 * Obsidian 阅读面同样不显示。嵌入卡片里「上线新版本 ^k1」原样露出是真机看到的（2026-10-04）。
 */
function withoutBlockId(children: MdInline[]): MdInline[] {
  const last = children[children.length - 1];
  if (last?.kind !== "text" || !BLOCK_ID_TAIL.test(last.text)) return children;
  const trimmed = last.text.replace(BLOCK_ID_TAIL, "");
  return trimmed ? [...children.slice(0, -1), { kind: "text", text: trimmed }] : children.slice(0, -1);
}

/** 遍历某容器节点的子级，产出块序列；未知类型回退为原文段落。 */
function blocksOf(container: SyntaxNode, text: string): MdBlock[] {
  const blocks: MdBlock[] = [];
  for (let child = container.firstChild; child; child = child.nextSibling) {
    const block = blockOf(child, text);
    if (block) {
      blocks.push(block);
    }
  }
  return blocks;
}

function blockOf(node: SyntaxNode, text: string): MdBlock | null {
  const level = HEADING_LEVELS[node.name];
  if (level) {
    return { kind: "heading", level, children: trimInlineEdges(inlinesOf(node, text)) };
  }
  switch (node.name) {
    case "Paragraph":
    case "Task": {
      const children = withoutBlockId(inlinesOf(node, text));
      const solid = children.filter((child) => child.kind !== "text" || child.text.trim() !== "");
      if (node.name === "Paragraph" && solid.length === 1 && solid[0].kind === "embed") {
        return { kind: "embed", target: solid[0].target, display: solid[0].display };
      }
      return { kind: "paragraph", children };
    }
    case "BlockMath":
      return { kind: "math", tex: mathSource(sliceOf(node, text), true) };
    case "FootnoteDefinition": {
      const label = findChild(node, "FootnoteLabel");
      return { kind: "footnote", id: label ? sliceOf(label, text) : "", children: inlinesOf(node, text) };
    }
    case "FencedCode":
    case "CodeBlock":
      return {
        kind: "code_block",
        ...codeBlockContentOf(node, (from, to) => text.slice(from, to)),
      };
    case "Blockquote":
      return blockquoteOf(node, text);
    case "BulletList":
    case "OrderedList":
      return {
        kind: "list",
        ordered: node.name === "OrderedList",
        items: listItemsOf(node, text),
      };
    case "Table":
      return tableOf(node, text);
    case "HorizontalRule":
      return { kind: "hr" };
    case "CommentBlock": {
      // 仅隐藏知识产物的严格声明边界。出处本身是行内标准 Markdown 链接，继续走
      // 共享阅读器的普通链接通道；其他用户注释仍按原文回退，不偷偷吞内容。
      const raw = sliceOf(node, text).trim();
      if (
        /^<!-- luna:claim:[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12} -->$/u.test(raw) ||
        raw === "<!-- /luna:claim -->"
      ) {
        return null;
      }
      return { kind: "paragraph", children: [{ kind: "text", text: raw }] };
    }
    // 标记类子级（引用 `>`、列表符号等）不产块。
    case "QuoteMark":
    case "ListMark":
    case "HeaderMark":
      return null;
    default: {
      // 未覆盖块类型（HTMLBlock/LinkReference…）：原文回退，绝不丢内容。
      const raw = sliceOf(node, text);
      if (!raw.trim()) {
        return null;
      }
      return { kind: "paragraph", children: [{ kind: "text", text: raw }] };
    }
  }
}

/**
 * 引用块 → 普通引用 或 标注块（PRD-06-011）。
 *
 * 判据落在**源文本首行**而非解析出的行内节点上：lezer 会把 `[!warning]` 当成链接标签
 * （`[` `]` 各成一个 LinkMark），首个行内节点因此不是纯文本，照节点判会一律漏判。
 *
 * 标注正文就是普通 Markdown，故剥掉一层引用标记后**交还同一个解析器**——列表、代码块、
 * 嵌套引用都按各自的规则解析，不需要为标注内部另写一套。
 */
function blockquoteOf(node: SyntaxNode, text: string): MdBlock {
  const lines = sliceOf(node, text).split("\n");
  const marker = parseCalloutMarker(stripQuoteLevel(lines[0] ?? ""));
  if (!marker) {
    return { kind: "blockquote", blocks: blocksOf(node, text) };
  }
  const title = stripQuoteLevel(lines[0]).slice(marker.markerEnd).replace(/^[ \t]/, "");
  const body = lines.slice(1).map(stripQuoteLevel).join("\n");
  return {
    kind: "callout",
    tone: marker.tone,
    label: marker.label,
    defaultFolded: marker.defaultFolded,
    title: title ? parseInlineMarkdown(title) : [],
    blocks: body.trim() ? parseChatMarkdown(body) : [],
  };
}

/**
 * 代码块节点（围栏 `FencedCode` / 缩进 `CodeBlock`）→ 语言 Info String + **正文本身**
 * （不含围栏行、不含缩进标记）。
 *
 * **两个渲染面共用**（TDD-15 §3.4a）：阅读面据此渲染 `<pre><code>`，编辑面的
 * 「复制代码」按钮据此取要送进剪贴板的字符串——"这块代码到底是哪几行"只能有一个答案，
 * 否则同一块代码在两处复制出的内容会不同（多一行围栏、少剥一层缩进都算不同）。
 */
export function codeBlockContentOf(
  node: SyntaxNode,
  /** 取文档片段。传函数而不是整篇文本：编辑面拿着 CM6 的 rope，`toString()` 一次全文
      只为读一个代码块是不必要的（每次文档变更都会走到这里）。 */
  read: (from: number, to: number) => string,
): { lang: string; code: string } {
  if (node.name !== "FencedCode") {
    // 缩进式代码块：逐行剥掉最多 4 个前导空格（缩进即语法标记）。
    return {
      lang: "",
      code: read(node.from, node.to)
        .split("\n")
        .map((line) => line.replace(/^ {1,4}/, ""))
        .join("\n"),
    };
  }
  let lang = "";
  let code = "";
  for (let child = node.firstChild; child; child = child.nextSibling) {
    if (child.name === "CodeInfo") {
      lang = read(child.from, child.to);
    } else if (child.name === "CodeText") {
      code = read(child.from, child.to);
    }
  }
  return { lang, code };
}

function listItemsOf(list: SyntaxNode, text: string): MdListItem[] {
  const items: MdListItem[] = [];
  for (let child = list.firstChild; child; child = child.nextSibling) {
    if (child.name !== "ListItem") {
      continue;
    }
    const item: MdListItem = { blocks: blocksOf(child, text) };
    const marker = findChild(child, "Task", "TaskMarker");
    if (marker) {
      item.task = sliceOf(marker, text).toLowerCase().includes("x") ? "done" : "todo";
    }
    items.push(item);
  }
  return items;
}

/** 在 `parent` 下按路径逐层找首个命名子节点。 */
function findChild(parent: SyntaxNode, ...path: string[]): SyntaxNode | null {
  let current: SyntaxNode | null = parent;
  for (const name of path) {
    let found: SyntaxNode | null = null;
    for (let child: SyntaxNode | null = current.firstChild; child; child = child.nextSibling) {
      if (child.name === name) {
        found = child;
        break;
      }
    }
    if (!found) {
      return null;
    }
    current = found;
  }
  return current;
}

/**
 * 图片的行内目标（`![alt](src "title")`）。
 *
 * **两个渲染面共用这一处判定**（TDD-15 §3.4a「同源要一路往回收到最上游那一处」）：
 * 阅读面走 lezer 树、编辑面此前走一条手写正则
 * `^!\[([^\]]*)\]\(([^)]+)\)$`——于是 `![a](x.png "标题")`、`![a](<带 空格.png>)`、
 * `![a](转义\)括号.png)`、alt 里带 `]` 的写法在编辑面**全不渲染或渲染出错误的 src**，
 * 阅读面却好好的。正则只会越写越长且永远追不上 CommonMark；判定改由**语法树**给出，
 * 两面于是不可能再分叉。
 */
export interface MdImageTarget {
  alt: string;
  src: string;
  /** `![说明|300](图.png)` 的显示宽度（像素，Obsidian 写法）；没写为 null。 */
  width: number | null;
}

const IMAGE_WIDTH = /^(.*?)\|(\d{1,4})(?:x\d{1,4})?$/su;

/**
 * 把 alt 里的宽度后缀拆出来（用户 2026-10-03 裁决用 Obsidian 写法 `|300`）：`说明|300` →
 * (`说明`, 300)；`|300` → (``, 300)；`300x200` 只取宽，高按比例。别的编辑器会把 `|300` 当作
 * alt 文字的一部分——图仍正常显示，只是说明里多了几个字。
 */
export function splitImageWidth(alt: string): { alt: string; width: number | null } {
  const match = IMAGE_WIDTH.exec(alt);
  if (!match) return { alt, width: null };
  const width = Number(match[2]);
  return width > 0 ? { alt: match[1].trim(), width } : { alt, width: null };
}

/**
 * 按绝对偏移取文本。字符串（阅读面已把整篇读进内存）与 CM6 的
 * `EditorState.sliceDoc`（编辑面文档是 rope，不该为取一段图片而整篇 toString）
 * 同签名——这个参数就是两面唯一的差异，判定本体不因载体分叉。
 */
export type SliceDoc = (from: number, to: number) => string;

/** alt 区里属于「语法」而非内容的节点：其覆盖的字符不进 alt。 */
const ALT_MARK_NODES = new Set([
  "LinkMark",
  "EmphasisMark",
  "CodeMark",
  "StrikethroughMark",
]);

/**
 * CommonMark 链接目标解码：脱去 `<…>` 包裹、还原 ASCII 标点的反斜杠转义。
 *
 * lezer 的 `URL` 节点给的是**源码原文**（含尖括号与反斜杠），而 asset 解析要的是
 * 真实相对路径——`<a b.png>` 要变成 `a b.png`、`a\)b.png` 要变成 `a)b.png`。
 * 不做百分号解码：那是 URL 层的事，Vault 路径按原样交给 Rust 侧解析。
 */
function decodeLinkDestination(raw: string): string {
  const inner = raw.startsWith("<") && raw.endsWith(">") ? raw.slice(1, -1) : raw;
  return inner.replace(/\\([!-/:-@[-`{-~])/gu, "$1");
}

/** 收集 alt 区内应被丢弃的字符区间（语法标记整段丢；转义只丢反斜杠）。 */
function collectAltDrops(node: SyntaxNode, until: number, out: Array<[number, number]>): void {
  for (let child = node.firstChild; child && child.from < until; child = child.nextSibling) {
    if (ALT_MARK_NODES.has(child.name)) {
      out.push([child.from, child.to]);
    } else if (child.name === "Escape") {
      out.push([child.from, child.from + 1]);
    } else {
      collectAltDrops(child, until, out);
    }
  }
}

/** alt = `![` 与目标区 `(` 之间的字符，减去其中的语法标记（CommonMark 的纯文本口径）。 */
function altTextOf(image: SyntaxNode, until: number, sliceDoc: SliceDoc): string {
  const drops: Array<[number, number]> = [];
  collectAltDrops(image, until, drops);
  drops.sort((a, b) => a[0] - b[0]);
  let out = "";
  let pos = image.from;
  for (const [from, to] of drops) {
    if (from > pos) {
      out += sliceDoc(pos, Math.min(from, until));
    }
    pos = Math.max(pos, to);
  }
  if (pos < until) {
    out += sliceDoc(pos, until);
  }
  return out;
}

/**
 * `Image` 语法节点 → `{alt, src}`；**没有行内目标**时返回 null。
 *
 * 返回 null 的三种现场都不是错误，而是「这段文字不是一张能就地渲染的图」：
 * 引用式 `![a][ref]`、空目标 `![a]()`、以及 Obsidian 内嵌 `![[note.png]]`
 * （维持 P2 不做，PRD-06-011）。调用方一律**静默按原文纯文本呈现**，不报错也不丢内容。
 */
export function imageTargetOf(image: SyntaxNode, sliceDoc: SliceDoc): MdImageTarget | null {
  const url = findChild(image, "URL");
  if (!url) {
    return null;
  }
  const src = decodeLinkDestination(sliceDoc(url.from, url.to)).trim();
  if (!src) {
    return null;
  }
  // alt 区右界 = 目标区起始 `(` 的位置（`(` 是 URL 之前最后一个子节点）。按整个 Image
  // 节点取 alt 会把 URL 与 `"title"` 之间的那个空格也算进 alt（阅读面此前就多了它）。
  let until = url.from;
  for (let child = image.firstChild; child && child.to <= url.from; child = child.nextSibling) {
    until = child.from;
  }
  return { ...splitImageWidth(altTextOf(image, until, sliceDoc)), src };
}

/**
 * GFM 分隔行 → 逐列对齐。冒号贴哪边就往哪边靠，两边都有即居中。
 *
 * 分隔行里不可能出现转义管道（它只由 `|`、`:`、`-`、空白构成），故这里的切分比
 * 编辑面的通用行切分（`table-model.ts` 的 `splitRowCells`，还要产出源码区间供光标
 * 落点用）简单——**判断本身只有这一处**，两面都调它。
 */
export function parseTableAlign(delimiterRow: string): TableAlign[] {
  return delimiterRow
    .trim()
    .replace(/^\|/u, "")
    .replace(/\|$/u, "")
    .split("|")
    .map((cell) => {
      const t = cell.trim();
      const left = t.startsWith(":");
      const right = t.endsWith(":");
      if (left && right) return "center";
      if (right) return "right";
      if (left) return "left";
      return null;
    });
}

function tableOf(node: SyntaxNode, text: string): MdBlock {
  let header: MdInline[][] = [];
  let align: TableAlign[] = [];
  const rows: MdInline[][][] = [];
  for (let child = node.firstChild; child; child = child.nextSibling) {
    if (child.name === "TableHeader") {
      header = cellsOf(child, text);
    } else if (child.name === "TableDelimiter") {
      align = parseTableAlign(sliceOf(child, text));
    } else if (child.name === "TableRow") {
      rows.push(cellsOf(child, text));
    }
  }
  return { kind: "table", header, align, rows };
}

function cellsOf(row: SyntaxNode, text: string): MdInline[][] {
  const cells: MdInline[][] = [];
  for (let child = row.firstChild; child; child = child.nextSibling) {
    if (child.name === "TableCell") {
      cells.push(inlinesOf(child, text));
    }
  }
  return cells;
}

/**
 * 行内内容：容器的具名子节点（强调/链接/行内代码…）+ 子节点之间的空隙文本。
 * 标记节点（`**`、反引号、`[` 等）跳过——其内容即空隙本身。
 */
function inlinesOf(container: SyntaxNode, text: string): MdInline[] {
  const out: MdInline[] = [];
  let pos = container.from;
  for (let child = container.firstChild; child; child = child.nextSibling) {
    if (child.from > pos) {
      pushText(out, text.slice(pos, child.from));
    }
    const inline = inlineOf(child, text);
    if (inline) {
      out.push(inline);
    }
    pos = Math.max(pos, child.to);
  }
  if (pos < container.to) {
    pushText(out, text.slice(pos, container.to));
  }
  return out;
}

/** 修剪行内序列首尾空白（标题 `#` 标记与正文间的空格属语法，不入内容）。 */
function trimInlineEdges(inlines: MdInline[]): MdInline[] {
  const out = [...inlines];
  const first = out[0];
  if (first?.kind === "text") {
    const trimmed = first.text.replace(/^\s+/, "");
    if (trimmed) {
      out[0] = { kind: "text", text: trimmed };
    } else {
      out.shift();
    }
  }
  const last = out[out.length - 1];
  if (last?.kind === "text") {
    const trimmed = last.text.replace(/\s+$/, "");
    if (trimmed) {
      out[out.length - 1] = { kind: "text", text: trimmed };
    } else {
      out.pop();
    }
  }
  return out;
}

function pushText(out: MdInline[], text: string) {
  if (text.length > 0) {
    out.push({ kind: "text", text });
  }
}

/** 放行的那一个 HTML 标签：`<br>` / `<br/>` / `<br />`（大小写不敏感）。 */
const LINE_BREAK_TAG = /^<br\s*\/?\s*>$/iu;

function inlineOf(node: SyntaxNode, text: string): MdInline | null {
  if (INLINE_MARKS.has(node.name)) {
    return null;
  }
  switch (node.name) {
    case "InlineMath":
      return { kind: "math", tex: mathSource(sliceOf(node, text), false) };
    case "FootnoteRef": {
      const label = findChild(node, "FootnoteLabel");
      return { kind: "footnote", id: label ? sliceOf(label, text) : "", note: null };
    }
    case "StrongEmphasis":
      return { kind: "strong", children: inlinesOf(node, text) };
    case "Emphasis":
      return { kind: "em", children: inlinesOf(node, text) };
    case "Strikethrough":
      return { kind: "del", children: inlinesOf(node, text) };
    case "InlineCode": {
      // 内容 = 标记之间的空隙；复用空隙法后拼接为纯文本。
      const parts = inlinesOf(node, text);
      return {
        kind: "code",
        text: parts.map((p) => (p.kind === "text" ? p.text : "")).join(""),
      };
    }
    case "Link": {
      const url = findChild(node, "URL");
      const href = url ? sliceOf(url, text) : "";
      const children = inlinesOf(node, text);
      return {
        kind: "link",
        href,
        children: children.length > 0 ? children : [{ kind: "text", text: href }],
      };
    }
    case "Image": {
      // 图片产出独立节点：渲染策略由消费方决定——对话按外链呈现（默认离线纪律，
      // 不远程加载）；笔记阅读模式对 Vault 相对路径经 asset 协议就地渲染（PRD-06-013）。
      // alt/src 的提取走**两面共用**的 `imageTargetOf`（TDD-15 §3.4a）。
      const image = imageTargetOf(node, (from, to) => text.slice(from, to));
      if (!image) {
        // 没有行内目标（引用式 / 空目标 / `![[内嵌]]`）：按原文纯文本呈现，静默不报错。
        return { kind: "text", text: sliceOf(node, text) };
      }
      return { kind: "image", src: image.src, alt: image.alt, width: image.width };
    }
    case "WikiLink":
      // 原文交给消费方：阅读面按同一条规则（`splitReadingInline`）切成可点的 wiki 链接，对话里照原文。
      return { kind: "text", text: sliceOf(node, text) };
    case "WikiEmbed": {
      const inner = findChild(node, "WikiEmbedTarget");
      return { kind: "embed", ...splitEmbed(inner ? sliceOf(inner, text) : "") };
    }
    case "Autolink": {
      const raw = sliceOf(node, text).replace(/^<|>$/g, "");
      return { kind: "link", href: raw, children: [{ kind: "text", text: raw }] };
    }
    case "Escape":
      // 反斜杠转义：产出被转义字符本身。
      return { kind: "text", text: sliceOf(node, text).slice(1) };
    case "HardBreak":
      return { kind: "text", text: "\n" };
    case "HTMLTag":
      return LINE_BREAK_TAG.test(sliceOf(node, text)) ? { kind: "break" } : { kind: "text", text: sliceOf(node, text) };
    default:
      // 未知行内类型：原文回退。
      return { kind: "text", text: sliceOf(node, text) };
  }
}

function sliceOf(node: SyntaxNode, text: string): string {
  return text.slice(node.from, node.to);
}

/** 行内序列拍平为纯文本（图片 alt 等展示性用途）。 */
export function inlineToPlainText(inlines: MdInline[]): string {
  return inlines
    .map((node) => {
      switch (node.kind) {
        case "break":
          return "\n";
        case "math":
          return node.tex;
        case "footnote":
          return "";
        case "text":
          return node.text;
        case "code":
          return node.text;
        case "image":
          return node.alt;
        case "embed":
          return node.display ?? node.target;
        default:
          return inlineToPlainText(node.children);
      }
    })
    .join("");
}

/** 仅允许 http/https 外链（`ChatMarkdown` 点击外开前置校验；其余协议一律不打开）。 */
export function isSafeExternalUrl(href: string): boolean {
  return /^https?:\/\//i.test(href);
}

/**
 * 一行 Markdown 压成**可读的一行**（摘录展示用）：链接只留它的文字、去掉强调与代码记号。
 *
 * 存在的理由：机器写进笔记的行常带链接语法，原样摆到卡片上用户读到的是
 * `[对话 2026-07-29 …](对话%20…%20·%20笔记.md)` —— 那是给解析器看的，不是给人看的。
 * 复用同一个解析基线（`parseInlineMarkdown`），故与阅读模式对"这行是什么"的理解一致。
 */
export function inlineMarkdownToText(line: string): string {
  return inlineToPlainText(parseInlineMarkdown(line)).trim();
}

/** 一段 Markdown 里全部嵌入的目标（`|` 之前，去重，文档顺序；代码里的不算）。 */
export function embedTargetsOf(text: string): string[] {
  const out: string[] = [];
  lunaMarkdownParser.parse(text).iterate({
    enter(node) {
      if (node.name !== "WikiEmbedTarget") return;
      const { target } = splitEmbed(text.slice(node.from, node.to));
      if (target && !out.includes(target)) out.push(target);
    },
  });
  return out;
}
