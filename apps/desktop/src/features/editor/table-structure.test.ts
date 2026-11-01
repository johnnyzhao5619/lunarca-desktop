import { describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { tableAt } from "./table-model";
import {
  alignColumnEdit,
  deleteColumnEdit,
  deleteColumnsEdit,
  deleteRowEdit,
  deleteRowsEdit,
  deleteTableEdit,
  insertColumnEdit,
  insertRowEdit,
  moveColumnEdit,
  moveColumnToEdit,
  moveRowEdit,
  moveRowToEdit,
  sortRowsEdit,
  tableToTextEdit,
  tidyTableEdit,
  type TableEdit,
} from "./table-structure";
import { fullyParsed } from "./syntax-tree-test-fixture";

/** 结构动作只改受影响的行（ADR-138 ② 2026-10-03 修订）：其余行的字节一个不动。 */
const TABLE = "| 名称 | 数量 |\n|:--- | ---: |\n| **乙** | 2 |\n| 甲 | 10 |";
const DOC = `前文\n\n${TABLE}\n\n后文`;

function setup(doc = DOC) {
  const state = fullyParsed(EditorState.create({ doc, extensions: [markdown({ base: markdownLanguage })] }));
  const model = tableAt(state, doc.indexOf("|") + 1)!;
  return { state, model };
}

function apply(state: EditorState, edit: TableEdit | { changes: TableEdit["changes"] } | null): string {
  expect(edit).not.toBeNull();
  return state.update({ changes: edit!.changes }).state.doc.toString();
}

describe("表格结构操作只改受影响的行", () => {
  it("插入行只插那一行；表头之下插在分隔行之后", () => {
    const { state, model } = setup();
    expect(apply(state, insertRowEdit(state, model, 1, 0))).toBe(DOC.replace("|:--- | ---: |\n", "|:--- | ---: |\n|  |  |\n"));
    expect(apply(state, insertRowEdit(state, model, 3, 0))).toBe(DOC.replace("| 甲 | 10 |", "| 甲 | 10 |\n|  |  |"));
    expect(insertRowEdit(state, model, 0, 0)).toBeNull();
  });

  it("删除正文行只删那一行，表头不可删", () => {
    const { state, model } = setup();
    expect(apply(state, deleteRowEdit(state, model, 1, 0))).toBe(DOC.replace("| **乙** | 2 |\n", ""));
    expect(deleteRowEdit(state, model, 0, 0)).toBeNull();
  });

  it("上移 / 下移只交换两行原文；表头不参与", () => {
    const { state, model } = setup();
    expect(apply(state, moveRowEdit(state, model, 2, -1, 0))).toBe(DOC.replace("| **乙** | 2 |\n| 甲 | 10 |", "| 甲 | 10 |\n| **乙** | 2 |"));
    expect(moveRowEdit(state, model, 1, -1, 0)).toBeNull();
  });

  it("插入 / 删除 / 移动列只动该列边界，分隔行同步", () => {
    const { state, model } = setup();
    expect(apply(state, insertColumnEdit(state, model, 1, 0))).toBe(
      DOC.replace("| 名称 | 数量 |", "| 名称 |  | 数量 |").replace("|:--- | ---: |", "|:--- | --- | ---: |")
        .replace("| **乙** | 2 |", "| **乙** |  | 2 |").replace("| 甲 | 10 |", "| 甲 |  | 10 |"),
    );
    expect(apply(state, deleteColumnEdit(state, model, 0, 0))).toBe(
      DOC.replace("| 名称 | 数量 |", "| 数量 |").replace("|:--- | ---: |", "| ---: |").replace("| **乙** | 2 |", "| 2 |").replace("| 甲 | 10 |", "| 10 |"),
    );
    expect(apply(state, moveColumnEdit(state, model, 0, 1, 0))).toBe(
      DOC.replace("| 名称 | 数量 |", "| 数量 | 名称 |").replace("|:--- | ---: |", "| ---: |:--- |")
        .replace("| **乙** | 2 |", "| 2 | **乙** |").replace("| 甲 | 10 |", "| 10 | 甲 |"),
    );
  });

  it("无行首管道的行在行首插列时整行改写成标准写法，不丢列", () => {
    const doc = "a | b\n--- | ---\n1 | 2";
    const { state, model } = setup(doc);
    expect(apply(state, insertColumnEdit(state, model, 0, 0))).toBe("|  | a | b |\n| --- | --- | --- |\n|  | 1 | 2 |");
  });

  it("对齐只改分隔行那一格", () => {
    const { state, model } = setup();
    expect(apply(state, alignColumnEdit(state, model, 0, "center", 0))).toBe(DOC.replace("|:--- |", "|:--: |"));
    expect(apply(state, alignColumnEdit(state, model, 1, null, 0))).toBe(DOC.replace("| ---: |", "| ---- |"));
    expect(alignColumnEdit(state, model, 0, "left", 0)).toBeNull();
  });

  it("排序按去掉标记的文字、数字按大小，只重排行", () => {
    const { state, model } = setup();
    expect(apply(state, sortRowsEdit(state, model, 1, true, "zh-CN"))).toBe(DOC.replace("| **乙** | 2 |\n| 甲 | 10 |", "| 甲 | 10 |\n| **乙** | 2 |"));
    expect(sortRowsEdit(state, model, 1, false, "zh-CN")).toBeNull();
    expect(apply(state, sortRowsEdit(state, model, 0, false, "zh-CN"))).toBe(DOC.replace("| **乙** | 2 |\n| 甲 | 10 |", "| 甲 | 10 |\n| **乙** | 2 |"));
  });

  it("连续几行 / 几列一次删掉；至少留一列，表头不删", () => {
    const { state, model } = setup();
    expect(apply(state, deleteRowsEdit(state, model, 1, 2, 0))).toBe(DOC.replace("\n| **乙** | 2 |\n| 甲 | 10 |", ""));
    expect(deleteRowsEdit(state, model, 0, 1, 0)).toBeNull();
    expect(deleteColumnsEdit(state, model, 0, 1, 0)).toBeNull();
    const wide = setup("| a | b | c |\n| - | - | - |\n| 1 | 2 | 3 |");
    expect(apply(wide.state, deleteColumnsEdit(wide.state, wide.model, 1, 2, 0))).toBe("| a |\n| - |\n| 1 |");
  });

  it("拖动行 / 列把手：移到目标位置，只改起止之间", () => {
    const doc = "| a | b | c |\n| :- | - | -: |\n| 1 | 2 | 3 |\n| 4 | 5 | 6 |\n| 7 | 8 | 9 |";
    const { state, model } = setup(doc);
    expect(apply(state, moveRowToEdit(state, model, 3, 1, 0))).toBe(doc.replace("| 1 | 2 | 3 |\n| 4 | 5 | 6 |\n| 7 | 8 | 9 |", "| 7 | 8 | 9 |\n| 1 | 2 | 3 |\n| 4 | 5 | 6 |"));
    expect(apply(state, moveColumnToEdit(state, model, 0, 2, 0))).toBe("| b | c | a |\n| - | -: | :- |\n| 2 | 3 | 1 |\n| 5 | 6 | 4 |\n| 8 | 9 | 7 |");
    expect(moveRowToEdit(state, model, 1, 0, 0)).toBeNull();
  });

  it("表格转为文本：制表符分列，Markdown 原样，<br> 还原为空格、\\| 还原为 |", () => {
    const { state, model } = setup("| a | b |\n| - | - |\n| **x**<br>y | p\\|q |");
    expect(apply(state, tableToTextEdit(state, model))).toBe("a\tb\n**x** y\tp|q");
  });

  it("删除整表带走一个换行；整理源码是唯一整表重写的动作", () => {
    const { state, model } = setup();
    expect(apply(state, deleteTableEdit(state, model))).toBe("前文\n\n\n后文");
    const tidy = apply(state, tidyTableEdit(model, 0, 0));
    expect(tidy).toContain("| 名称   | 数量 |");
  });
});
