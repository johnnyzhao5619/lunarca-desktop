// 列表 / 引用块编辑手感（PRD-06-014 MVP-1 列表项，Obsidian / Notion 对标）：项首退格、
// Shift+Enter 软换行、Tab / Shift-Tab 按列表结构缩进、有序列表自动重编号、引用块的续写与
// 退出。全部是 state 级纯函数（可无头测试）+ 薄命令包装。
//
// **本仓库关掉了 `@codemirror/lang-markdown` 自带的键位**（`markdown({ addKeymap: false })`，
// 见 VaultEditor）：它的 `insertNewlineContinueMarkup` / `deleteMarkupBackward` 挂在 `Prec.high`，
// 压过本文件与 `editor-extensions.ts` 的全部绑定——项首退格时它把列表标记**换成等宽空格**，
// `- [ ] 待办` 一次退格变成六个空格开头的 `待办`，恰好是缩进代码块（真机 2026-09-04 第三轮
// "带复制按钮的盒子"）。它承担过的引用块续写 / 退格在本文件补齐，不留功能缺口。
//
// 判据按**行文本**而不是语法树：这些命令在键入的瞬间执行，而 lezer 是带预算的增量解析，
// 刚敲下的那一行未必已经在树里（`listContinuationForState` 同一取舍）。文本判据在 CommonMark
// 的列表规则下是精确的：标记 + 至少一个空格；嵌套关系只看前导空格；引用前缀 `> ` 对列表
// 结构透明。围栏代码块例外——`- foo` 在代码里是代码，交给 `isInsideFencedCode`。
//
// 重编号只在**这些命令**之后做，不在每次键入后做：用户把 `3.` 手改成 `1.` 是在表达
// 起始编号（CommonMark 认它），键入时自动改回去是在和用户抢键盘。命令产生的结构变化
// （插项 / 删项 / 升降级）才是"编号该跟着走"的时刻——Obsidian 同一取舍。

import {
  type ChangeSet,
  type ChangeSpec,
  type EditorSelection,
  type EditorState,
  type Extension,
  type Line,
  type TransactionSpec,
} from "@codemirror/state";
import { type EditorView, keymap } from "@codemirror/view";
import { isInsideFencedCode } from "./code-fence-editing";
import { singleCursorOnly } from "./multi-cursor";

/** 一条列表行的文本结构。 */
export interface ListLine {
  /** 引用前缀（`> ` / `> > `），非引用为空串。对列表结构透明，只在重建前缀时原样带上。 */
  quote: string;
  /** 前导空格（嵌套层级）。 */
  indent: string;
  /** 标记本身：`-` / `*` / `+` / `1.` / `1)`。 */
  marker: string;
  /** 有序编号；无序为 null。 */
  number: number | null;
  /** 标记后的空格。 */
  gap: string;
  /** 任务框连同其后空格（`[ ] `）；非任务为空串。 */
  task: string;
  body: string;
}

const QUOTE_PREFIX_RE = /^((?:[ \t]{0,3}>[ \t]?)*)/u;
const LIST_LINE_RE = /^([ \t]*)([-*+]|\d{1,9}[.)])([ \t]+)(\[[ xX]\][ \t]+)?(.*)$/u;

/** 拆出行首的引用前缀（CommonMark：每层 `>` 前至多三个空格、后至多一个空格）。 */
export function splitQuote(text: string): { quote: string; rest: string } {
  const quote = QUOTE_PREFIX_RE.exec(text)?.[1] ?? "";
  return { quote, rest: text.slice(quote.length) };
}

export function parseListLine(text: string): ListLine | null {
  const { quote, rest } = splitQuote(text);
  const match = LIST_LINE_RE.exec(rest);
  if (!match) {
    return null;
  }
  const [, indent, marker, gap, task = "", body] = match;
  const number = /^\d/u.test(marker) ? Number.parseInt(marker, 10) : null;
  return { quote, indent, marker, number, gap, task, body };
}

/** 标记前缀总长（引用前缀 + 前导空格 + 标记 + 空格 + 任务框）= 正文起点的列。 */
export function listPrefixLength(line: ListLine): number {
  return listMarkerEnd(line) + line.task.length;
}

/** 引用前缀 + 前导空格 + 标记 + 其后空格的总长（任务框之前）。 */
function listMarkerEnd(line: ListLine): number {
  return line.quote.length + line.indent.length + line.marker.length + line.gap.length;
}

/** 去掉引用前缀后的前导空格数——嵌套关系只看它。 */
function leadingSpaces(text: string): number {
  return /^[ \t]*/u.exec(splitQuote(text).rest)?.[0].length ?? 0;
}

/** 去掉引用前缀后为空即空行（引用块里单独一个 `>` 也算）。 */
function isBlank(text: string): boolean {
  return splitQuote(text).rest.trim() === "";
}

/**
 * 父项的缩进列：向上找最近一条缩进更浅的列表行。找不到即顶层（0）。
 * 途中遇到顶格的非列表正文即视为列表已结束。
 */
export function parentIndentOf(state: EditorState, lineNumber: number, indent: number): number {
  for (let n = lineNumber - 1; n >= 1; n--) {
    const text = state.doc.line(n).text;
    if (isBlank(text)) {
      continue;
    }
    const lead = leadingSpaces(text);
    const item = parseListLine(text);
    if (item && lead < indent) {
      return lead;
    }
    if (!item && lead === 0) {
      break;
    }
  }
  return 0;
}

/**
 * 升一级要缩进多少：CommonMark 里嵌套项要对齐到**上一个同级项**的正文列，而 `- ` 与 `1. `
 * 的标记宽度不同（2 / 3 / `10. ` 4）。固定 2 空格会让 `1.` 下的 Tab 产出一个同级项而不是
 * 子项。找不到同级项时按本行自己的标记宽计（首项升级：结构不变、只是视觉上进一格）。
 */
export function indentStepOf(state: EditorState, lineNumber: number, self: ListLine): number {
  const indent = self.indent.length;
  for (let n = lineNumber - 1; n >= 1; n--) {
    const text = state.doc.line(n).text;
    if (isBlank(text)) {
      continue;
    }
    const lead = leadingSpaces(text);
    const item = parseListLine(text);
    if (item && lead === indent) {
      return item.marker.length + item.gap.length;
    }
    if ((item && lead < indent) || (!item && lead === 0)) {
      break;
    }
  }
  return self.marker.length + self.gap.length;
}

/** 一项连同它的子项 / 续行（后续缩进更深的行；夹在中间的空行随其后的深行一起算）。 */
export function listItemExtent(state: EditorState, lineNumber: number): { from: number; to: number } {
  const first = state.doc.line(lineNumber);
  const indent = leadingSpaces(first.text);
  let last = lineNumber;
  for (let n = lineNumber + 1; n <= state.doc.lines; n++) {
    const text = state.doc.line(n).text;
    if (isBlank(text)) {
      // 空行只在后面还有更深的行时才属于本项。
      let next = n + 1;
      while (next <= state.doc.lines && isBlank(state.doc.line(next).text)) {
        next++;
      }
      if (next > state.doc.lines || leadingSpaces(state.doc.line(next).text) <= indent) {
        break;
      }
      continue;
    }
    if (leadingSpaces(text) <= indent) {
      break;
    }
    last = n;
  }
  return { from: lineNumber, to: last };
}

/**
 * 有序列表重编号：以 `lineNumber` 所在的连续列表块为范围，同一缩进列上的有序项按首项编号
 * 连续递增；无序项打断该列的计数，更浅的项结束所有更深列的计数；空行与续行不打断
 * （松散列表）。只改编号数字，分隔符 `.` / `)` 照旧。
 */
export function renumberChanges(state: EditorState, lineNumber: number): ChangeSpec[] {
  const doc = state.doc;
  const endsList = (text: string) => !isBlank(text) && leadingSpaces(text) === 0 && !parseListLine(text);
  let start = lineNumber;
  while (start > 1 && !endsList(doc.line(start - 1).text)) {
    start--;
  }
  let end = lineNumber;
  while (end < doc.lines && !endsList(doc.line(end + 1).text)) {
    end++;
  }
  const changes: ChangeSpec[] = [];
  const counters = new Map<number, number>();
  for (let n = start; n <= end; n++) {
    const line = doc.line(n);
    const item = parseListLine(line.text);
    if (!item) {
      continue;
    }
    const indent = item.indent.length;
    for (const deeper of [...counters.keys()]) {
      if (deeper > indent) {
        counters.delete(deeper);
      }
    }
    if (item.number === null) {
      counters.delete(indent);
      continue;
    }
    const previous = counters.get(indent);
    // 顶层列的起始编号照旧（CommonMark 认它，用户可能有意从 3 起）；嵌套列一律从 1 起——
    // 嵌套列几乎都是升降级命令造出来的，"2." 打头的子列表没有人想要。
    const expected = previous === undefined ? (indent > 0 ? 1 : item.number) : previous + 1;
    counters.set(indent, expected);
    if (expected !== item.number) {
      const from = line.from + item.quote.length + indent;
      changes.push({ from, to: from + String(item.number).length, insert: String(expected) });
    }
  }
  return changes;
}

/**
 * 把一组变更与其后的重编号合成**一个**事务：两次 dispatch 会成为两个撤销步（重编号的改动
 * 与原改动不相邻，历史不合并），撤销一次只撤回编号、列表还是断的。
 * `selectionAfter` 以第一组变更之后的坐标给出。
 */
export function withRenumber(
  state: EditorState,
  changes: ChangeSpec,
  selectionAfter: EditorSelection,
): TransactionSpec {
  const first: ChangeSet = state.changes(changes);
  const mid = state.update({ changes: first, selection: selectionAfter }).state;
  const touched = new Set<number>();
  for (const range of mid.selection.ranges) {
    touched.add(mid.doc.lineAt(range.from).number);
    touched.add(mid.doc.lineAt(range.to).number);
  }
  const specs: ChangeSpec[] = [];
  const seen = new Set<number>();
  for (const lineNumber of touched) {
    for (const spec of renumberChanges(mid, lineNumber)) {
      const key = (spec as { from: number }).from;
      if (!seen.has(key)) {
        seen.add(key);
        specs.push(spec);
      }
    }
  }
  const fix = mid.changes(specs);
  return {
    changes: first.compose(fix),
    selection: mid.selection.map(fix),
    userEvent: "input",
  };
}

/** 当前主选区是光标点且不在围栏代码里时给出所在行；否则 null。 */
function caretLine(state: EditorState): Line | null {
  const selection = state.selection.main;
  if (!selection.empty || isInsideFencedCode(state, selection.head)) {
    return null;
  }
  return state.doc.lineAt(selection.head);
}

/**
 * 项首退格（Notion / Obsidian 口径）：光标在正文起点时——任务项先去任务框（`- [ ] ` → `- `）；
 * 嵌套项降一级；顶层项去掉标记变成普通段落（引用块里的项去掉标记后仍留在引用里）。
 * 三步各按一次退格，每一步都可见、可撤销。引用行的正文起点退格 = 去掉一层 `> `。
 * 都不是 → null，交给默认退格。
 */
export function listBackspaceSpec(state: EditorState): TransactionSpec | null {
  const line = caretLine(state);
  if (!line) {
    return null;
  }
  const head = state.selection.main.head;
  const item = parseListLine(line.text);
  if (!item) {
    const { quote } = splitQuote(line.text);
    if (quote.length === 0 || head !== line.from + quote.length) {
      return null;
    }
    // 只去最里一层 `>`：`> > |` 退一次变 `> |`。逐段顺序匹配再取末段——尾锚定的正则会把
    // 上一层的尾随空格也算进最里一层。
    const units = quote.match(/[ \t]{0,3}>[ \t]?/gu) ?? [];
    const inner = units.length > 0 ? units[units.length - 1].length : quote.length;
    return {
      changes: { from: line.from + quote.length - inner, to: line.from + quote.length, insert: "" },
      selection: { anchor: line.from + quote.length - inner },
      userEvent: "delete",
    };
  }
  const markerEnd = line.from + listMarkerEnd(item);
  const contentStart = line.from + listPrefixLength(item);
  if (item.task) {
    // 任务框在 Live 里是个复选框 widget，点在正文首字前面光标可能落在框的任一侧
    // （`- [ ]| 待办` 或 `- [ ] |待办`）：整段任务框区间都算"项首"，否则退格会从
    // 框里抠掉一个字符、把任务项拆成 `- [ 待办`（真机 2026-09-04 第二轮）。
    if (head < markerEnd || head > contentStart) {
      return null;
    }
    return {
      changes: { from: markerEnd, to: contentStart, insert: "" },
      selection: { anchor: markerEnd },
      userEvent: "delete",
    };
  }
  if (head !== contentStart) {
    return null;
  }
  const lineBody = line.from + item.quote.length;
  const remove =
    item.indent.length > 0
      ? { from: lineBody, to: lineBody + item.indent.length - parentIndentOf(state, line.number, item.indent.length), insert: "" }
      : { from: lineBody, to: contentStart, insert: "" };
  return withRenumber(state, remove, state.selection.map(state.changes(remove)));
}

/**
 * Shift+Enter：在项内换一行但不另起一项（续行对齐到正文列，Markdown 里是同一段的软换行）；
 * 引用行则带着 `> ` 换行。不在列表 / 引用行 → null，交给默认处理。
 */
export function listSoftBreakSpec(state: EditorState): TransactionSpec | null {
  const line = caretLine(state);
  if (!line) {
    return null;
  }
  const head = state.selection.main.head;
  const item = parseListLine(line.text);
  let insert: string;
  if (item) {
    insert = `\n${item.quote}${" ".repeat(listPrefixLength(item) - item.quote.length)}`;
  } else {
    const { quote } = splitQuote(line.text);
    if (quote.length === 0) {
      return null;
    }
    insert = `\n${quote}`;
  }
  return {
    changes: { from: head, to: head, insert },
    selection: { anchor: head + insert.length },
    userEvent: "input",
  };
}

/** 选区覆盖的行号区间（非空选区尾恰在行首时不算那一行，与显隐判定同一半开口径）。 */
function selectedLineRange(state: EditorState): { from: number; to: number } {
  const { from, to } = state.selection.main;
  const first = state.doc.lineAt(from).number;
  const lastLine = state.doc.lineAt(to);
  const last = to > from && lastLine.from === to ? lastLine.number - 1 : lastLine.number;
  return { from: first, to: Math.max(first, last) };
}

/**
 * Tab / Shift-Tab：选区触及的每个列表项**连同子项**升 / 降一级，升级步长按同级项的标记宽算，
 * 降级回到父项的缩进列；之后重编号。选区里有非列表行 → null（交给通用缩进）。
 */
export function listIndentSpec(state: EditorState, outdent: boolean): TransactionSpec | null {
  if (isInsideFencedCode(state, state.selection.main.head)) {
    return null;
  }
  const { from, to } = selectedLineRange(state);
  const items: Array<{ lineNumber: number; item: ListLine }> = [];
  for (let n = from; n <= to; n++) {
    const text = state.doc.line(n).text;
    if (isBlank(text)) {
      continue;
    }
    const item = parseListLine(text);
    if (!item) {
      // 续行（缩进的非列表行）随所属项一起动；顶格正文则不是列表操作。
      if (leadingSpaces(text) === 0) {
        return null;
      }
      continue;
    }
    items.push({ lineNumber: n, item });
  }
  if (items.length === 0) {
    return null;
  }
  const changes: ChangeSpec[] = [];
  const done = new Set<number>();
  for (const { lineNumber, item } of items) {
    if (done.has(lineNumber)) {
      continue;
    }
    const indent = item.indent.length;
    const step = outdent
      ? indent - parentIndentOf(state, lineNumber, indent)
      : indentStepOf(state, lineNumber, item);
    if (step <= 0) {
      continue;
    }
    const extent = listItemExtent(state, lineNumber);
    for (let n = extent.from; n <= extent.to; n++) {
      done.add(n);
      const line = state.doc.line(n);
      if (isBlank(line.text)) {
        continue;
      }
      const at = line.from + splitQuote(line.text).quote.length;
      if (outdent) {
        const remove = Math.min(step, leadingSpaces(line.text));
        changes.push({ from: at, to: at + remove, insert: "" });
      } else {
        changes.push({ from: at, to: at, insert: " ".repeat(step) });
      }
    }
  }
  if (changes.length === 0) {
    return null;
  }
  return withRenumber(state, changes, state.selection.map(state.changes(changes)));
}

function runSpec(view: EditorView, spec: TransactionSpec | null): boolean {
  if (!spec) {
    return false;
  }
  view.dispatch(spec);
  return true;
}

/** 列表编辑键位；置于表格 Tab 之后、通用缩进 / 默认退格之前。 */
export function listEditingKeymap(): Extension {
  return keymap.of([
    { key: "Backspace", run: singleCursorOnly((view) => runSpec(view, listBackspaceSpec(view.state))) },
    { key: "Shift-Enter", run: singleCursorOnly((view) => runSpec(view, listSoftBreakSpec(view.state))) },
    { key: "Tab", run: singleCursorOnly((view) => runSpec(view, listIndentSpec(view.state, false))) },
    { key: "Shift-Tab", run: singleCursorOnly((view) => runSpec(view, listIndentSpec(view.state, true))) },
  ]);
}
