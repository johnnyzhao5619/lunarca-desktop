import { describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { parseTableAlign } from "../markdown/markdown";
import { collectTables, splitRowCells, tableAt } from "./table-model";
import { buildTablePreviewDecorations, TablePreviewWidget } from "./table-preview";
import { formatTableText, insertTableSpec, tableAdvanceSpec } from "./table-edit";

const TABLE = "| 名称 | 数量 |\n| --- | ---: |\n| 苹果 | 3 |\n| 梨 | 12 |";

function mdState(doc: string, cursor = 0): EditorState {
  return EditorState.create({
    doc,
    selection: { anchor: cursor },
    extensions: [markdown({ base: markdownLanguage })],
  });
}

function widgetCount(state: EditorState): number {
  const set = buildTablePreviewDecorations(state);
  let count = 0;
  const iter = set.iter();
  while (iter.value) {
    if (iter.value.spec.widget instanceof TablePreviewWidget) {
      count += 1;
    }
    iter.next();
  }
  return count;
}

describe("table-model：行切分与对齐解析", () => {
  it("外围管道剥除、单元格内容与源码区间对应", () => {
    const cells = splitRowCells("| a | bb |", 100);
    expect(cells.map((c) => c.text)).toEqual(["a", "bb"]);
    expect(cells[0].from).toBe(102);
    expect(cells[0].to).toBe(103);
    expect(cells[1].text.length).toBe(cells[1].to - cells[1].from);
  });

  it("空单元格保留落点位置", () => {
    const cells = splitRowCells("| a ||", 0);
    expect(cells).toHaveLength(2);
    expect(cells[1].text).toBe("");
    expect(cells[1].from).toBeGreaterThanOrEqual(cells[1].segFrom);
  });

  it("转义管道不切分", () => {
    const cells = splitRowCells("| a \\| b | c |", 0);
    expect(cells.map((c) => c.text)).toEqual(["a \\| b", "c"]);
  });
  it("GFM 无管道正文行保留文字及真实源范围，不显示成空行", () => {
    const state = mdState(`${TABLE}\n最终保存护栏本地内容`);
    const tables = collectTables(state, 0, state.doc.length);
    const rows = tables[0].rows;
    const cell = rows[rows.length - 1].cells[0];
    expect(cell.text).toBe("最终保存护栏本地内容");
    expect(state.sliceDoc(cell.from, cell.to)).toBe(cell.text);
    expect(widgetCount(state)).toBe(1);
  });

  it("对齐声明解析（left/center/right/默认）", () => {
    expect(parseTableAlign("| :-- | :-: | --: | --- |")).toEqual([
      "left",
      "center",
      "right",
      null,
    ]);
  });

  it("collectTables 解析表头/正文行并排除代码块内伪表格", () => {
    const state = mdState(`${TABLE}\n\n\`\`\`\n| x | y |\n| - | - |\n\`\`\``);
    const tables = collectTables(state, 0, state.doc.length);
    expect(tables).toHaveLength(1);
    expect(tables[0].rows).toHaveLength(3); // 表头 + 2 行正文（分隔行不算）。
    expect(tables[0].align).toEqual([null, "right"]);
  });

  it("tableAt 光标在表格内命中、表外为 null", () => {
    const doc = `前文\n\n${TABLE}\n\n后文`;
    const state = mdState(doc);
    expect(tableAt(state, doc.indexOf("苹果"))).not.toBeNull();
    expect(tableAt(state, 0)).toBeNull();
  });
});

describe("表格 Live Preview（PRD-06-011 P1，选区显隐）", () => {
  const doc = `前文\n\n${TABLE}\n\n后文`;

  it("选区不触及表格时渲染表格 widget", () => {
    expect(widgetCount(mdState(doc, 0))).toBe(1);
  });

  it("光标进入单元格后保持表格布局", () => {
    expect(widgetCount(mdState(doc, doc.indexOf("苹果")))).toBe(1);
  });

  it("引用块内表格保持源码（不渲染 widget）", () => {
    const quoted = "> | a | b |\n> | - | - |\n> | 1 | 2 |";
    expect(widgetCount(mdState(quoted, 0))).toBe(0);
  });
});

describe("表格编辑：Tab 导航 + 对齐格式化（Obsidian 心智）", () => {
  it("Tab 跳到同行下一单元格，不改写任何源码字节", () => {
    const doc = `| 名称 | 数量 |\n| --- | --- |\n| 苹果 | 3 |`;
    const state = mdState(doc, doc.indexOf("苹果") + 2);
    const spec = tableAdvanceSpec(state, false);
    expect(spec).not.toBeNull();
    // 光标落在"3"之后（下一单元格内容末尾）。
    expect(doc.slice(0, spec!.anchor).endsWith("3")).toBe(true);
    expect(spec!.insert).toBe("");
    expect(spec!.from).toBe(spec!.to);
  });

  it("末行末格 Tab 只在表尾追加一行空行，不重排整表", () => {
    const doc = `| a | b |\n| - | - |\n| 1 | 2 |`;
    const state = mdState(doc, doc.length - 3);
    const spec = tableAdvanceSpec(state, false);
    expect(spec).not.toBeNull();
    expect({ from: spec!.from, to: spec!.to, insert: spec!.insert }).toEqual({ from: doc.length, to: doc.length, insert: "\n|  |  |" });
  });

  it("Shift-Tab 跳回上一单元格；首格返回 null 落回默认", () => {
    const doc = `| a | b |\n| - | - |\n| 1 | 2 |`;
    const inSecond = mdState(doc, doc.lastIndexOf("2"));
    expect(tableAdvanceSpec(inSecond, true)).not.toBeNull();
    const atFirst = mdState(doc, doc.indexOf("a"));
    expect(tableAdvanceSpec(atFirst, true)).toBeNull();
  });

  it("光标不在表格内返回 null（落回缩进语义）", () => {
    const state = mdState("普通段落", 2);
    expect(tableAdvanceSpec(state, false)).toBeNull();
  });

  it("CJK 单元格按全角宽度对齐", () => {
    const formatted = formatTableText(
      [
        ["中文", "b"],
        ["x", "y"],
      ],
      [null, null],
      2,
    );
    const lines = formatted.text.split("\n");
    expect(new Set(lines.map((l) => l.length)).size).not.toBe(1); // 字符数不同…
    // …但第一列右边界（第二个管道位置的展示列）一致由宽度补齐保证：
    // "中文" 展示宽 4 → "x" 补 3 空格。
    expect(lines[2]).toContain("| x    |");
  });
});

describe("插入表格骨架", () => {
  it("空行处就地插入，光标进首个表头单元格", () => {
    const state = mdState("", 0);
    const spec = insertTableSpec(state);
    expect(spec.insert.startsWith("|")).toBe(true);
    expect(spec.anchor).toBeGreaterThan(spec.from);
  });

  it("非空行处先换行再插入", () => {
    const doc = "正文";
    const state = mdState(doc, doc.length);
    const spec = insertTableSpec(state);
    expect(spec.insert.startsWith("\n\n|")).toBe(true);
    expect(spec.from).toBe(doc.length);
  });

  it("下文已是空行时不再补结尾换行（不留两个空行）；下文紧贴时照补", () => {
    const blankBelow = mdState("\n\n后文", 0);
    expect(insertTableSpec(blankBelow).insert.endsWith("|")).toBe(true);
    const textBelow = mdState("\n后文", 0);
    expect(insertTableSpec(textBelow).insert.endsWith("|\n")).toBe(true);
    const atEnd = mdState("前文\n", 3);
    expect(insertTableSpec(atEnd).insert.endsWith("|\n")).toBe(true);
  });
});
