import { describe, expect, it } from "vitest";
import { EditorSelection, EditorState, type TransactionSpec } from "@codemirror/state";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import {
  indentStepOf,
  listBackspaceSpec,
  listIndentSpec,
  listItemExtent,
  listSoftBreakSpec,
  parentIndentOf,
  parseListLine,
  renumberChanges,
  withRenumber,
} from "./list-editing";

/** `|` 标光标；`«` `»` 标选区（方括号留给任务框）。 */
function stateOf(marked: string): EditorState {
  const anchorIdx = marked.indexOf("«");
  if (anchorIdx >= 0) {
    const headIdx = marked.indexOf("»") - 1;
    const doc = marked.replace("«", "").replace("»", "");
    return EditorState.create({
      doc,
      selection: { anchor: anchorIdx, head: headIdx },
      extensions: [markdown({ base: markdownLanguage })],
    });
  }
  const cursor = marked.indexOf("|");
  return EditorState.create({
    doc: marked.replace("|", ""),
    selection: { anchor: cursor },
    extensions: [markdown({ base: markdownLanguage })],
  });
}

/** 应用事务规格，返回 `|` 标光标的新文档（多选区只标主选区）。 */
function apply(state: EditorState, spec: TransactionSpec | null): string | null {
  if (!spec) {
    return null;
  }
  const next = state.update(spec).state;
  const { from, to } = next.selection.main;
  const text = next.doc.toString();
  return from === to
    ? `${text.slice(0, from)}|${text.slice(from)}`
    : `${text.slice(0, from)}«${text.slice(from, to)}»${text.slice(to)}`;
}

describe("parseListLine", () => {
  it("无序 / 有序 / 任务 / 缩进", () => {
    expect(parseListLine("- 甲")).toEqual({ quote: "", indent: "", marker: "-", number: null, gap: " ", task: "", body: "甲" });
    expect(parseListLine("  10) 乙")).toEqual({ quote: "", indent: "  ", marker: "10)", number: 10, gap: " ", task: "", body: "乙" });
    expect(parseListLine("> > - 引用里")?.quote).toBe("> > ");
    expect(parseListLine("- [x] 丙")?.task).toBe("[x] ");
    expect(parseListLine("-没空格")).toBeNull();
    expect(parseListLine("正文")).toBeNull();
  });
});

describe("结构判定", () => {
  const doc = "1. 甲\n   - 子一\n   - 子二\n\n     续行\n2. 乙";
  const state = stateOf(doc + "|");

  it("父项缩进列 / 升级步长按同级项的标记宽（`1. ` 下嵌套要 3 空格）", () => {
    expect(parentIndentOf(state, 3, 3)).toBe(0);
    expect(indentStepOf(state, 6, parseListLine("2. 乙")!)).toBe(3);
    expect(indentStepOf(state, 3, parseListLine("   - 子二")!)).toBe(2);
    // 首项没有同级项：按自己的标记宽。
    expect(indentStepOf(state, 1, parseListLine("1. 甲")!)).toBe(3);
  });

  it("一项的范围含子项与隔空行的续行", () => {
    expect(listItemExtent(state, 1)).toEqual({ from: 1, to: 5 });
    expect(listItemExtent(state, 3)).toEqual({ from: 3, to: 5 });
    expect(listItemExtent(state, 6)).toEqual({ from: 6, to: 6 });
  });
});

describe("有序列表重编号", () => {
  it("同一列连续递增，起始编号照旧，分隔符不动", () => {
    const state = stateOf("3. 甲\n5) 乙\n9. 丙|");
    const next = state.update({ changes: renumberChanges(state, 2) }).state;
    expect(next.doc.toString()).toBe("3. 甲\n4) 乙\n5. 丙");
  });

  it("嵌套各列独立；更浅的项结束深列计数；无序项打断本列", () => {
    const state = stateOf("1. 甲\n   1. 子\n   7. 子\n2. 乙|\n   1. 又从一开始\n- 无序\n9. 重新起头");
    const next = state.update({ changes: renumberChanges(state, 4) }).state;
    expect(next.doc.toString()).toBe("1. 甲\n   1. 子\n   2. 子\n2. 乙\n   1. 又从一开始\n- 无序\n9. 重新起头");
  });

  it("松散列表（项间空行）不打断；顶格正文结束列表块", () => {
    const state = stateOf("1. 甲\n\n1. 乙|\n\n正文\n\n1. 另一个列表\n1. 的第二项");
    const next = state.update({ changes: renumberChanges(state, 3) }).state;
    expect(next.doc.toString()).toBe("1. 甲\n\n2. 乙\n\n正文\n\n1. 另一个列表\n1. 的第二项");
  });
});

describe("项首退格（三步各一次）", () => {
  it("任务项先去任务框；光标落在框的任一侧都算项首", () => {
    expect(apply(stateOf("- [ ] |待办"), listBackspaceSpec(stateOf("- [ ] |待办")))).toBe("- |待办");
    expect(apply(stateOf("- [ ]| 待办"), listBackspaceSpec(stateOf("- [ ]| 待办")))).toBe("- |待办");
    expect(apply(stateOf("- |[ ] 待办"), listBackspaceSpec(stateOf("- |[ ] 待办")))).toBe("- |待办");
  });

  it("嵌套项降一级，后续编号跟着走（留下的子列表从 1 起）", () => {
    const state = stateOf("1. 甲\n   1. |子\n   2. 子二\n2. 乙");
    expect(apply(state, listBackspaceSpec(state))).toBe("1. 甲\n2. |子\n   1. 子二\n3. 乙");
  });

  it("顶层项去标记变段落；余下的顶层编号照旧（起始编号是作者的）", () => {
    const state = stateOf("- 甲\n- |乙");
    expect(apply(state, listBackspaceSpec(state))).toBe("- 甲\n|乙");
    const ordered = stateOf("1. |甲\n2. 乙\n3. 丙");
    expect(apply(ordered, listBackspaceSpec(ordered))).toBe("|甲\n2. 乙\n3. 丙");
  });

  it("光标不在正文起点 → 交给默认退格", () => {
    expect(listBackspaceSpec(stateOf("- 甲|"))).toBeNull();
    expect(listBackspaceSpec(stateOf("|- 甲"))).toBeNull();
    expect(listBackspaceSpec(stateOf("正文|"))).toBeNull();
  });
});

describe("Shift+Enter 软换行", () => {
  it("续行对齐到正文列（含任务框）", () => {
    expect(apply(stateOf("- [ ] 待办|"), listSoftBreakSpec(stateOf("- [ ] 待办|")))).toBe("- [ ] 待办\n      |");
    expect(apply(stateOf("  10. 乙|"), listSoftBreakSpec(stateOf("  10. 乙|")))).toBe("  10. 乙\n      |");
  });

  it("不在列表行 → null", () => {
    expect(listSoftBreakSpec(stateOf("正文|"))).toBeNull();
  });
});

describe("Tab / Shift-Tab 按列表结构缩进", () => {
  it("有序项下 Tab 缩进 3 空格（成为子项，不是同级）", () => {
    const state = stateOf("1. 甲\n2. |乙");
    expect(apply(state, listIndentSpec(state, false))).toBe("1. 甲\n   1. |乙");
  });

  it("整项连同子项一起动；Shift-Tab 回到父项的缩进列并重编号", () => {
    const state = stateOf("1. 甲\n   1. |子\n      - 孙\n   2. 子二\n2. 乙");
    expect(apply(state, listIndentSpec(state, true))).toBe("1. 甲\n2. |子\n   - 孙\n   1. 子二\n3. 乙");
  });

  it("选区覆盖多项时逐项升级；选区尾恰在行首不算那一行", () => {
    const state = stateOf("- 甲\n«- 乙\n- 丙\n»- 丁");
    expect(apply(state, listIndentSpec(state, false))).toBe("- 甲\n  «- 乙\n  - 丙\n»- 丁");
  });

  it("顶层项 Shift-Tab 无事可做 → null；选区含顶格正文 → null（交给通用缩进）", () => {
    expect(listIndentSpec(stateOf("- |甲"), true)).toBeNull();
    expect(listIndentSpec(stateOf("«- 甲\n正文»"), false)).toBeNull();
  });
});

describe("引用块（接替 lang-markdown 自带键位）", () => {
  it("引用里的列表项：退格去标记后仍留在引用里，续行带 `> `", () => {
    const state = stateOf("> - |甲");
    expect(apply(state, listBackspaceSpec(state))).toBe("> |甲");
    expect(apply(stateOf("> - 甲|"), listSoftBreakSpec(stateOf("> - 甲|")))).toBe("> - 甲\n>   |");
  });

  it("引用行正文起点退格去最里一层 `>`", () => {
    expect(apply(stateOf("> > |甲"), listBackspaceSpec(stateOf("> > |甲")))).toBe("> |甲");
    expect(apply(stateOf("> |甲"), listBackspaceSpec(stateOf("> |甲")))).toBe("|甲");
    expect(listBackspaceSpec(stateOf("> 甲|"))).toBeNull();
  });

  it("引用里的有序列表重编号按引用后的列算", () => {
    const state = stateOf("> 1. 甲\n> 1. |乙");
    const next = state.update({ changes: renumberChanges(state, 2) }).state;
    expect(next.doc.toString()).toBe("> 1. 甲\n> 2. 乙");
  });
});

describe("withRenumber：一个事务", () => {
  it("在有序列表中间插项，后续编号跟着走且合成单个变更集", () => {
    const state = stateOf("1. 甲|\n2. 乙\n3. 丙");
    const spec = withRenumber(state, { from: 4, to: 4, insert: "\n2. " }, EditorSelection.single(8));
    const next = state.update(spec).state;
    expect(next.doc.toString()).toBe("1. 甲\n2. \n3. 乙\n4. 丙");
    expect(next.selection.main.head).toBe(8);
  });
});
