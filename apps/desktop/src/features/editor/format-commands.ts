// Markdown 格式命令层（PRD-06-014「编辑器基础生产力」，Obsidian/Typora 对标）：
// 右键菜单与格式工具栏共用的**单一命令来源**——行内标记真 toggle（已加粗再点即
// 去加粗）、标题设置/切换、行级前缀 toggle（列表/任务/引用）、块插入（代码块/
// 分隔线/wiki 链接）与光标处格式态检测（工具栏激活高亮）。
// 与 table-edit 同构：纯函数产出「变更规格」（可无头测试），view 包装只 dispatch。

import { syntaxTree } from "@codemirror/language";
import { EditorSelection, type ChangeSpec, type EditorState } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";

// ─── 行内标记真 toggle ───────────────────────────────────────────────────────

export type InlineMark = "strong" | "em" | "del" | "code";

const INLINE_MARKERS: Record<InlineMark, string> = {
  strong: "**",
  em: "*",
  del: "~~",
  code: "`",
};

/** 行内标记 → lezer 元素节点名（触达检测与 unwrap 定位共用）。 */
const INLINE_NODE_NAMES: Record<InlineMark, string> = {
  strong: "StrongEmphasis",
  em: "Emphasis",
  del: "Strikethrough",
  code: "InlineCode",
};

interface InlineToggleSpec {
  changes: ChangeSpec[] | ChangeSpec;
  anchor: number;
  head: number;
}

/** 覆盖 [from, to] 的目标行内元素节点（用于 unwrap；找不到返回 null）。 */
function enclosingInlineNode(
  state: EditorState,
  from: number,
  to: number,
  mark: InlineMark,
): { from: number; to: number } | null {
  const name = INLINE_NODE_NAMES[mark];
  for (const side of [1, -1] as const) {
    // 两侧各试一次：光标贴元素右边界时 resolveInner(from, 1) 会落到元素外。
    const node = syntaxTree(state).resolveInner(side === 1 ? from : to, side);
    for (let cur: typeof node | null = node; cur; cur = cur.parent) {
      if (cur.name === name && cur.from <= from && cur.to >= to) {
        return { from: cur.from, to: cur.to };
      }
    }
  }
  return null;
}

/**
 * 单个选区的行内标记 toggle 规格（纯函数）：选区在该标记元素内 → 剥掉两侧标记
 * （真 toggle）；否则包裹选区（空选区插入标记对、光标置中）。
 */
export function inlineToggleSpec(
  state: EditorState,
  from: number,
  to: number,
  mark: InlineMark,
): InlineToggleSpec {
  const marker = INLINE_MARKERS[mark];
  const enclosing = enclosingInlineNode(state, from, to, mark);
  if (enclosing) {
    const len = marker.length;
    return {
      changes: [
        { from: enclosing.from, to: enclosing.from + len },
        { from: enclosing.to - len, to: enclosing.to },
      ],
      anchor: Math.max(enclosing.from, from - len),
      head: Math.max(enclosing.from, to - len),
    };
  }
  const selected = state.doc.sliceString(from, to);
  return {
    changes: { from, to, insert: `${marker}${selected}${marker}` },
    anchor: from + marker.length,
    head: from + marker.length + selected.length,
  };
}

export function toggleInlineMark(view: EditorView, mark: InlineMark): boolean {
  const state = view.state;
  const transaction = state.changeByRange((range) => {
    const spec = inlineToggleSpec(state, range.from, range.to, mark);
    return {
      changes: spec.changes,
      range: EditorSelection.range(spec.anchor, spec.head),
    };
  });
  view.dispatch(state.update(transaction, { userEvent: "input" }));
  view.focus();
  return true;
}

// ─── 行级前缀（标题 / 列表 / 任务 / 引用） ───────────────────────────────────

const HEADING_PREFIX_RE = /^(#{1,6})[ \t]+/u;
const BULLET_PREFIX_RE = /^([ \t]*)([-*+])[ \t]+(\[[ xX]\][ \t]+)?/u;
const ORDERED_PREFIX_RE = /^([ \t]*)\d+[.)][ \t]+/u;
const QUOTE_PREFIX_RE = /^>[ \t]?/u;

export type LineBlockKind = "bullet" | "ordered" | "task" | "quote";

export type HeadingLevel = 0 | 1 | 2 | 3 | 4 | 5 | 6;

/** 各选区覆盖的行（多光标时取并集、按文档顺序、同一行只算一次；B5）。 */
function selectedLines(state: EditorState): { number: number; from: number; to: number; text: string }[] {
  const numbers = new Set<number>();
  for (const range of state.selection.ranges) {
    const last = state.doc.lineAt(range.to).number;
    for (let n = state.doc.lineAt(range.from).number; n <= last; n++) numbers.add(n);
  }
  return [...numbers]
    .sort((a, b) => a - b)
    .map((n) => {
      const line = state.doc.line(n);
      return { number: n, from: line.from, to: line.to, text: line.text };
    });
}

/** 剥掉一行的块级前缀（标题/列表/任务/引用中的最外层一种），返回纯正文。 */
function stripLinePrefix(text: string): string {
  for (const re of [HEADING_PREFIX_RE, BULLET_PREFIX_RE, ORDERED_PREFIX_RE, QUOTE_PREFIX_RE]) {
    const m = re.exec(text);
    if (m) {
      return text.slice(m[0].length);
    }
  }
  return text;
}

/** 某行当前的标题级别（非标题为 0）。 */
export function headingLevelOf(text: string): number {
  const m = HEADING_PREFIX_RE.exec(text);
  return m ? m[1].length : 0;
}

/**
 * 设置选中行标题级别的变更规格（纯函数，Obsidian 心智）：行已是该级别 → 还原
 * 正文（toggle）；`level` 0 = 显式还原正文；列表/引用前缀被替换（标题是行级排版）。
 */
export function setHeadingChanges(state: EditorState, level: HeadingLevel): ChangeSpec[] {
  const changes: ChangeSpec[] = [];
  for (const line of selectedLines(state)) {
    const body = stripLinePrefix(line.text);
    const already = headingLevelOf(line.text);
    const next =
      already === level || level === 0 ? body : `${"#".repeat(level)} ${body}`;
    if (next !== line.text) {
      changes.push({ from: line.from, to: line.to, insert: next });
    }
  }
  return changes;
}

export function setHeading(view: EditorView, level: HeadingLevel): boolean {
  const changes = setHeadingChanges(view.state, level);
  if (changes.length > 0) {
    view.dispatch({ changes, userEvent: "input" });
  }
  view.focus();
  return true;
}

/** 行是否已具备目标块前缀。 */
export function lineHasBlockPrefix(text: string, kind: LineBlockKind): boolean {
  switch (kind) {
    case "task": {
      const m = BULLET_PREFIX_RE.exec(text);
      return m !== null && m[3] !== undefined;
    }
    case "bullet": {
      const m = BULLET_PREFIX_RE.exec(text);
      return m !== null && m[3] === undefined;
    }
    case "ordered":
      return ORDERED_PREFIX_RE.test(text);
    case "quote":
      return QUOTE_PREFIX_RE.test(text);
    default:
      return false;
  }
}

/**
 * 行级块 toggle 的变更规格（纯函数）：选中行**全部**已是目标形态 → 还原正文；
 * 否则统一转为目标形态（既有其它前缀被替换；有序列表自动编号）。多行时空行跳过。
 */
export function toggleLineBlockChanges(state: EditorState, kind: LineBlockKind): ChangeSpec[] {
  const all = selectedLines(state);
  const lines = all.length > 1 ? all.filter((l) => l.text.trim() !== "") : all;
  if (lines.length === 0) {
    return [];
  }
  const allApplied = lines.every((l) => lineHasBlockPrefix(l.text, kind));
  const changes: ChangeSpec[] = [];
  let ordinal = 1;
  const selected = new Set(all.map((l) => l.number));
  for (const line of lines) {
    // 有序编号按连续选中的行计（中间的空行算连着）：多光标分处几段时，每段各自从 1 起。
    if (!selected.has(line.number - 1)) ordinal = 1;
    const body = stripLinePrefix(line.text);
    let next = body;
    if (!allApplied) {
      switch (kind) {
        case "bullet":
          next = `- ${body}`;
          break;
        case "task":
          next = `- [ ] ${body}`;
          break;
        case "ordered":
          next = `${ordinal}. ${body}`;
          ordinal += 1;
          break;
        case "quote":
          next = `> ${body}`;
          break;
        default:
          break;
      }
    }
    if (next !== line.text) {
      changes.push({ from: line.from, to: line.to, insert: next });
    }
  }
  return changes;
}

export function toggleLineBlock(view: EditorView, kind: LineBlockKind): boolean {
  const changes = toggleLineBlockChanges(view.state, kind);
  if (changes.length > 0) {
    view.dispatch({ changes, userEvent: "input" });
  }
  view.focus();
  return true;
}

// ─── 块插入（代码块 / 分隔线 / wiki 链接） ──────────────────────────────────

interface InsertSpec {
  from: number;
  to: number;
  insert: string;
  anchor: number;
  head: number;
}

/** 围栏代码块插入规格（纯函数）：选区包成代码块；空选区插空块、光标进块内。 */
export function codeBlockSpec(state: EditorState): InsertSpec {
  const range = state.selection.main;
  if (!range.empty) {
    const selected = state.doc.sliceString(range.from, range.to);
    const anchor = range.from + 4; // ```\n 之后
    return {
      from: range.from,
      to: range.to,
      insert: `\`\`\`\n${selected}\n\`\`\``,
      anchor,
      head: anchor + selected.length,
    };
  }
  const line = state.doc.lineAt(range.head);
  const onEmptyLine = line.text.trim() === "";
  const at = onEmptyLine ? range.head : line.to;
  const prefix = onEmptyLine ? "" : "\n";
  const anchor = at + prefix.length + 4;
  return { from: at, to: at, insert: `${prefix}\`\`\`\n\n\`\`\``, anchor, head: anchor };
}

/**
 * 标注块插入规格（纯函数，PRD-06-011）：选区逐行加引用前缀并顶一行 `> [!note] `，
 * 光标落在类型标记之后好接着写标题；空选区插一个待填的空标注块。
 *
 * 类型默认 `note`——插入的是**语法骨架**，改成 warning/tip 只需改那个词；给一份下拉
 * 选类型属于第二套入口（工具栏已有位置有限），先让语法可发现即可。
 */
export function calloutSpec(state: EditorState): InsertSpec {
  const range = state.selection.main;
  const head = "> [!note] ";
  if (!range.empty) {
    const fromLine = state.doc.lineAt(range.from);
    const toLine = state.doc.lineAt(range.to);
    const body = state.doc
      .sliceString(fromLine.from, toLine.to)
      .split("\n")
      .map((line) => `> ${line}`)
      .join("\n");
    const insert = `${head}\n${body}`;
    const anchor = fromLine.from + head.length;
    return { from: fromLine.from, to: toLine.to, insert, anchor, head: anchor };
  }
  const line = state.doc.lineAt(range.head);
  const onEmptyLine = line.text.trim() === "";
  const at = onEmptyLine ? line.from : line.to;
  const prefix = onEmptyLine ? "" : "\n\n";
  const insert = `${prefix}${head}\n> `;
  const anchor = at + prefix.length + head.length;
  return { from: at, to: at, insert, anchor, head: anchor };
}

/** 水平分隔线插入规格（纯函数）：独占一行；非空行则另起空行后插入。 */
export function horizontalRuleSpec(state: EditorState): InsertSpec {
  const line = state.doc.lineAt(state.selection.main.head);
  const onEmptyLine = line.text.trim() === "";
  const at = onEmptyLine ? line.from : line.to;
  const insert = onEmptyLine ? "---\n" : "\n\n---\n";
  return { from: at, to: at, insert, anchor: at + insert.length, head: at + insert.length };
}

function applyInsert(view: EditorView, spec: InsertSpec): boolean {
  view.dispatch({
    changes: { from: spec.from, to: spec.to, insert: spec.insert },
    selection: EditorSelection.range(spec.anchor, spec.head),
    userEvent: "input",
  });
  view.focus();
  return true;
}

export function insertCodeBlock(view: EditorView): boolean {
  return applyInsert(view, codeBlockSpec(view.state));
}

export function insertHorizontalRule(view: EditorView): boolean {
  return applyInsert(view, horizontalRuleSpec(view.state));
}

export function insertCallout(view: EditorView): boolean {
  return applyInsert(view, calloutSpec(view.state));
}

/** 选区包成 wiki 链接 `[[…]]`（空选区把光标置于括号中间触发既有补全）。 */
export function insertWikiLink(view: EditorView): boolean {
  const state = view.state;
  const transaction = state.changeByRange((range) => {
    const selected = state.doc.sliceString(range.from, range.to);
    const anchor = range.from + 2;
    return {
      changes: { from: range.from, to: range.to, insert: `[[${selected}]]` },
      range: EditorSelection.range(anchor, anchor + selected.length),
    };
  });
  view.dispatch(state.update(transaction, { userEvent: "input" }));
  view.focus();
  return true;
}

// ─── 光标处格式态检测（工具栏激活高亮 / 菜单勾选） ──────────────────────────

export interface FormatState {
  strong: boolean;
  em: boolean;
  del: boolean;
  code: boolean;
  /** 光标行标题级别（非标题为 0）。 */
  heading: number;
  bullet: boolean;
  ordered: boolean;
  task: boolean;
  quote: boolean;
}

/** 主选区处的格式态（state 级纯函数，可无头测试）。 */
export function formatStateAt(state: EditorState): FormatState {
  const range = state.selection.main;
  const line = state.doc.lineAt(range.head);
  const inlineActive = (mark: InlineMark) =>
    enclosingInlineNode(state, range.from, range.to, mark) !== null;
  return {
    strong: inlineActive("strong"),
    em: inlineActive("em"),
    del: inlineActive("del"),
    code: inlineActive("code"),
    heading: headingLevelOf(line.text),
    bullet: lineHasBlockPrefix(line.text, "bullet"),
    ordered: lineHasBlockPrefix(line.text, "ordered"),
    task: lineHasBlockPrefix(line.text, "task"),
    quote: lineHasBlockPrefix(line.text, "quote"),
  };
}
