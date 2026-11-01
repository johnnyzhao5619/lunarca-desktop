// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import { syntaxTree } from "@codemirror/language";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { history, undo } from "@codemirror/commands";
import { EditorView, runScopeHandlers } from "@codemirror/view";
import { tableCellTargetOf, tablePreviewExtension } from "./table-preview";
import { tableMenuItems, type TableMenuLabels } from "./table-menu";
import { applyTableSpec, insertTableCommand, separateFromTableAboveSpec, tableAdvanceSpec } from "./table-edit";

/** 菜单文案替身：键名即文案，断言按 id 找条目。 */
const LABELS = new Proxy({}, { get: (_target, key) => String(key) }) as TableMenuLabels;

const doc = "前文\n\n| 名称 | 数量 |\n| --- | --- |\n| 苹果 | 3 |\n\n后文";
const views: EditorView[] = [];
let copyRange: (markdown: string) => void = () => {};
let openMenu: NonNullable<NonNullable<Parameters<typeof tablePreviewExtension>[0]>["openMenu"]> = () => {};
function mount(content = doc, pasteCell?: NonNullable<Parameters<typeof tablePreviewExtension>[0]>["pasteCell"]) {
  const parent = document.createElement("div"); document.body.appendChild(parent);
  const view = new EditorView({ parent, state: EditorState.create({ doc: content,
    extensions: [history(), markdown({ base: markdownLanguage }), tablePreviewExtension({
      cell: (r, c) => `${r}/${c}`, rowHandle: (r) => `Row ${r}`, columnHandle: (c) => `Column ${c}`, appendRow: "Append row", appendColumn: "Append column", pasteCell, openMenu, copyRange: (markdown) => copyRange(markdown),
    })] }) });
  views.push(view); return view;
}
afterEach(() => { for (const view of views.splice(0)) view.destroy(); document.body.replaceChildren(); });
describe("direct table input", () => {
  it("矩形粘贴使用单元格入口；纯文本快捷键保留浏览器选区与同一份撤销", () => {
    const pasteCell = vi.fn(async () => false); const view = mount(doc, pasteCell);
    const input = view.dom.querySelector<HTMLTextAreaElement>('textarea[data-row="1"][data-column="0"]')!;
    input.focus();
    const paste = () => {
      const event = new Event("paste", { bubbles: true, cancelable: true });
      Object.defineProperty(event, "clipboardData", { value: { getData: (type: string) => type === "text/plain" ? "A\tB" : "" } });
      input.dispatchEvent(event); return event;
    };
    expect(paste().defaultPrevented).toBe(true); expect(pasteCell).toHaveBeenCalledOnce();
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "v", metaKey: true, shiftKey: true, bubbles: true }));
    expect(paste().defaultPrevented).toBe(false); expect(pasteCell).toHaveBeenCalledOnce();
    input.value = "纯文本\n两行";
    input.dispatchEvent(new InputEvent("input", { inputType: "insertFromPaste", bubbles: true }));
    expect(view.state.doc.toString()).toBe(doc.replace("苹果", "纯文本<br>两行"));
    input.blur(); expect(undo(view)).toBe(true); expect(view.state.doc.toString()).toBe(doc);
  });
  it("输入只改一格，保留 DOM 与聚焦输入，撤销走同一 CM 文档", () => {
    const view = mount();
    const input = view.dom.querySelector<HTMLTextAreaElement>('textarea[data-row="1"][data-column="0"]')!;
    input.focus(); input.value = "梨"; input.dispatchEvent(new Event("input"));
    expect(view.state.doc.toString()).toBe(doc.replace("苹果", "梨"));
    expect(view.dom.querySelector('textarea[data-row="1"][data-column="0"]')).toBe(input);
    expect(undo(view)).toBe(true);
    expect(view.state.doc.toString()).toBe(doc);
    expect(input.value).toBe("苹果");
  });
  it("空格子第一次写入整段换成 ` 文字 `，不从格段正中插进去；继续输入照常改内容", () => {
    const content = "| 名称 | 数量 |\n| --- | --- |\n|     | 3 |";
    const view = mount(content);
    const input = view.dom.querySelector<HTMLTextAreaElement>('textarea[data-row="1"][data-column="0"]')!;
    input.focus(); input.value = "a"; input.dispatchEvent(new Event("input"));
    expect(view.state.doc.line(3).text).toBe("| a | 3 |");
    input.value = "a/"; input.dispatchEvent(new Event("input"));
    expect(view.state.doc.line(3).text).toBe("| a/ | 3 |");
  });
  it("组合输入结束前不改源文档，结束后一次提交；导航不占撤销历史", () => {
    const view = mount();
    const input = view.dom.querySelector<HTMLTextAreaElement>('textarea[data-row="1"][data-column="0"]')!;
    input.focus(); input.dispatchEvent(new Event("compositionstart"));
    input.value = "拼音候选"; input.dispatchEvent(new Event("input"));
    expect(view.state.doc.toString()).toBe(doc);
    input.dispatchEvent(new Event("compositionend"));
    expect(view.state.doc.toString()).toContain("拼音候选");
    applyTableSpec(view, tableAdvanceSpec(view.state, false));
    expect(undo(view)).toBe(true);
    expect(view.state.doc.toString()).toBe(doc);
  });
  it("文末表格按两次 Escape 创建正文落点，段落边界可撤销", () => {
    const content = doc.replace("\n\n后文", ""); const view = mount(content);
    const input = view.dom.querySelector<HTMLTextAreaElement>('textarea[data-row="1"][data-column="0"]')!;
    input.focus(); input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(view.state.doc.toString()).toBe(content);
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(view.state.doc.toString()).toBe(`${content}\n\n`);
    expect(view.state.selection.main.head).toBe(view.state.doc.length);
    expect(document.activeElement).toBe(view.contentDOM);
    // happy-dom 在撤销的 DOM 更新中同步派发 selectionchange；浏览器在更新后派发。
    view.contentDOM.blur();
    expect(undo(view)).toBe(true); expect(view.state.doc.toString()).toBe(content);
  });
  it.each(["Escape", "Tab", "Enter"])("WKWebView 组字结束的 %s 不导航或改写，之后独立按键仍可执行", async (key) => {
    const view = mount();
    const input = view.dom.querySelector<HTMLTextAreaElement>('textarea[data-row="1"][data-column="0"]')!;
    input.focus();
    input.dispatchEvent(new CompositionEvent("compositionstart"));
    const end = new CompositionEvent("compositionend", { data: "" });
    Object.defineProperty(end, "timeStamp", { value: 100 });
    input.dispatchEvent(end);
    const consumed = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
    Object.defineProperty(consumed, "timeStamp", { value: 101 });
    input.dispatchEvent(consumed);
    expect(consumed.defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(input);
    expect(view.state.doc.toString()).toBe(doc);
    const independent = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
    Object.defineProperty(independent, "timeStamp", { value: 200 });
    input.dispatchEvent(independent);
    await Promise.resolve();
    // 独立的 Escape 进网格态（不离开），Tab 换格，末行的 Enter 新增一行（ADR-138 2026-10-03 修订）。
    if (key === "Escape") expect(input.dataset.grid).toBe("true");
    else if (key === "Tab") expect(document.activeElement).not.toBe(input);
    else expect(view.state.doc.lines).toBe(doc.split("\n").length + 1);
  });
  it("输入法 keyCode 229 的 Escape 不占用候选取消", () => {
    const view = mount();
    const input = view.dom.querySelector<HTMLTextAreaElement>('textarea[data-row="1"][data-column="0"]')!;
    input.focus();
    const event = new KeyboardEvent("keydown", { key: "Escape", keyCode: 229, bubbles: true, cancelable: true });
    input.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(input);
    expect(view.state.doc.toString()).toBe(doc);
  });
});

/** 表格 T0 缺陷（2026-10-03 对标方案 §3）：每条都是此前一定会发生的错，不是边界情况。 */
describe("table T0 regressions", () => {
  const cellInput = (view: EditorView, row: number, col: number) =>
    view.dom.querySelector<HTMLTextAreaElement>(`textarea[data-row="${row}"][data-column="${col}"]`)!;
  const type = (input: HTMLTextAreaElement, value: string) => {
    input.value = value;
    input.dispatchEvent(new Event("input"));
  };
  const key = (input: HTMLTextAreaElement, name: string, shiftKey = false) => {
    const event = new KeyboardEvent("keydown", { key: name, shiftKey, bubbles: true, cancelable: true });
    input.dispatchEvent(event);
    return event;
  };

  it("T-1 敲空格不被重绘吃掉：New York 不会变成 NewYork", () => {
    const view = mount();
    const input = cellInput(view, 1, 0);
    input.focus();
    type(input, "New");
    type(input, "New ");
    expect(input.value).toBe("New ");
    type(input, "New York");
    expect(view.state.doc.toString()).toBe(doc.replace("苹果", "New York"));
  });

  it("T-2 Escape 不落在紧贴表格的行：表格下有空行就落到空行之后，不改正文", () => {
    const view = mount();
    const input = cellInput(view, 1, 0);
    input.focus();
    key(input, "Escape");
    key(input, "Escape");
    expect(view.state.doc.toString()).toBe(doc);
    expect(view.state.doc.lineAt(view.state.selection.main.head).text).toBe("后文");
  });

  it("T-2 紧贴表格的空行：挪成独立段落，下一行不空时两侧都留空行；别处不动", () => {
    const state = EditorState.create({ doc, extensions: [markdown({ base: markdownLanguage })] });
    const blank = state.doc.line(6);
    expect(separateFromTableAboveSpec(state, blank.from)).toEqual({ from: blank.from, to: blank.from, insert: "\n\n", anchor: blank.from + 1 });
    const atEnd = EditorState.create({ doc: doc.replace("\n\n后文", "\n"), extensions: [markdown({ base: markdownLanguage })] });
    expect(separateFromTableAboveSpec(atEnd, atEnd.doc.length)).toEqual({ from: atEnd.doc.length, to: atEnd.doc.length, insert: "\n", anchor: atEnd.doc.length + 1 });
    expect(separateFromTableAboveSpec(state, state.doc.line(2).from)).toBeNull();
    expect(separateFromTableAboveSpec(state, state.doc.line(7).from)).toBeNull();
  });

  it("T-2 语法树没解析到表格（时间片用尽）也认得出表格，不随解析进度时有时无", () => {
    // 解析器按 Date.now 计时间片：每读一次跳 1 秒，新建状态的 20 ms 首轮解析只做完一小截。
    let now = 0;
    const clock = vi.spyOn(Date, "now").mockImplementation(() => (now += 1000));
    const state = EditorState.create({ doc, selection: { anchor: doc.indexOf("苹果") }, extensions: [markdown({ base: markdownLanguage })] });
    clock.mockRestore();
    expect(syntaxTree(state).length).toBeLessThan(doc.indexOf("|"));
    const blank = state.doc.line(6);
    expect(separateFromTableAboveSpec(state, blank.from)).toEqual({ from: blank.from, to: blank.from, insert: "\n\n", anchor: blank.from + 1 });
    expect(tableAdvanceSpec(state, false)).not.toBeNull();
  });

  it("T-2 在紧贴表格的空行上打字 / 粘贴不并进表格，一次撤销连同挪动一起回去", () => {
    const view = mount();
    const blank = view.state.doc.line(6).from;
    view.dispatch({ selection: { anchor: blank } });
    view.contentDOM.dispatchEvent(new KeyboardEvent("keydown", { key: "/", bubbles: true, cancelable: true }));
    view.dispatch({ changes: { from: view.state.selection.main.head, insert: "/" }, selection: { anchor: view.state.selection.main.head + 1 }, userEvent: "input.type" });
    expect(view.state.doc.toString()).toBe(doc.replace("\n\n后文", "\n\n/\n\n后文"));
    undo(view);
    expect(view.state.doc.toString()).toBe(doc);
    view.dispatch({ selection: { anchor: blank } });
    view.contentDOM.dispatchEvent(new KeyboardEvent("keydown", { key: "b", metaKey: true, bubbles: true, cancelable: true }));
    expect(view.state.doc.toString()).toBe(doc);
    view.contentDOM.dispatchEvent(new Event("paste", { bubbles: true, cancelable: true }));
    expect(view.state.doc.toString()).toBe(doc.replace("\n\n后文", "\n\n\n\n后文"));
  });

  it("T-2 表格后只剩一个空行时补一个换行，落点与表格隔着空行", () => {
    const content = doc.replace("\n\n后文", "\n");
    const view = mount(content);
    cellInput(view, 1, 0).focus();
    key(cellInput(view, 1, 0), "Escape");
    key(cellInput(view, 1, 0), "Escape");
    expect(view.state.doc.toString()).toBe(`${content}\n`);
    const line = view.state.doc.lineAt(view.state.selection.main.head);
    expect(line.text).toBe("");
    expect(view.state.doc.line(line.number - 1).text).toBe("");
  });

  it("T-4 短行缺的格可以写，只补这一行的管道", () => {
    const content = "| 名称 | 数量 | 备注 |\n| --- | --- | --- |\n| 苹果 | 3 |\n\n后文";
    const view = mount(content);
    const missing = cellInput(view, 1, 2);
    expect(missing).not.toBeNull();
    missing.focus();
    type(missing, "当季");
    expect(view.state.doc.toString()).toBe(content.replace("| 苹果 | 3 |", "| 苹果 | 3 | 当季 |"));
  });

  it("T-4 Tab 走进缺的格再走出来，不卡在原格", async () => {
    const content = "| 名称 | 数量 | 备注 |\n| --- | --- | --- |\n| 苹果 | 3 |\n| 梨 | 1 | 好 |\n\n后文";
    const view = mount(content);
    cellInput(view, 1, 1).focus();
    key(cellInput(view, 1, 1), "Tab");
    await Promise.resolve();
    expect(document.activeElement).toBe(cellInput(view, 1, 2));
    key(cellInput(view, 1, 2), "Tab");
    await Promise.resolve();
    expect(document.activeElement).toBe(cellInput(view, 2, 0));
    expect(view.state.doc.toString()).toBe(content);
  });

  it("T-5 插入表格后焦点直接在首个表头格，可以马上打字", async () => {
    const view = mount("段落\n\n");
    view.dispatch({ selection: { anchor: view.state.doc.length } });
    insertTableCommand(view);
    await Promise.resolve();
    expect(document.activeElement).toBe(cellInput(view, 0, 0));
  });

  it("T-7 首格 Shift-Tab 停在原处，焦点不外逃", async () => {
    const view = mount();
    const first = cellInput(view, 0, 0);
    first.focus();
    expect(key(first, "Tab", true).defaultPrevented).toBe(true);
    await Promise.resolve();
    expect(document.activeElement).toBe(first);
  });

  it("T-6 表格菜单作用于右键的那一格，而不是编辑器选区", () => {
    const content = "| A | B |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |\n\n后文";
    const view = mount(content);
    const target = tableCellTargetOf(cellInput(view, 2, 1))!;
    view.dispatch({ selection: { anchor: 0 } }); // 右键把选区挪到别处
    const items = tableMenuItems({ view, target, labels: LABELS, locale: "zh-CN", onCopy: () => {} });
    const deleteRow = items.find((item) => item.kind === "action" && item.id === "delete-row");
    expect(deleteRow?.kind === "action" && !deleteRow.disabled).toBe(true);
    if (deleteRow?.kind === "action") deleteRow.onSelect();
    expect(view.state.doc.toString()).toBe(content.replace("| 3 | 4 |\n", ""));
  });
});

/** T1 键盘模型（ADR-138 2026-10-03 修订）：编辑态 / 网格态两态，方向键在格边跨格并能离开表格。 */
describe("table T1 keyboard model", () => {
  const cellInput = (view: EditorView, row: number, col: number) =>
    view.dom.querySelector<HTMLTextAreaElement>(`textarea[data-row="${row}"][data-column="${col}"]`)!;
  const key = (target: HTMLElement, name: string, init: KeyboardEventInit = {}) => {
    const event = new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true, ...init });
    target.dispatchEvent(event);
    return event;
  };
  const twoRows = "前文\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |\n\n后文";
  const focused = () => document.activeElement as HTMLTextAreaElement;
  const coords = () => [focused().dataset.row, focused().dataset.column].join(",");

  it("Enter 跳到下一行同一列，末行时新增一行；⇧Enter 留给格内换行", async () => {
    const view = mount(twoRows);
    cellInput(view, 1, 1).focus();
    expect(key(cellInput(view, 1, 1), "Enter").defaultPrevented).toBe(true);
    await Promise.resolve();
    expect(coords()).toBe("2,1");
    expect(key(focused(), "Enter", { shiftKey: true }).defaultPrevented).toBe(false);
    key(focused(), "Enter");
    await Promise.resolve();
    expect(coords()).toBe("3,1");
    expect(view.state.doc.toString().split("\n").filter((l) => l.startsWith("|"))).toHaveLength(5);
  });

  it("⌘Enter 在当前行下方插入一行并进入同一列", async () => {
    const view = mount(twoRows);
    cellInput(view, 1, 0).focus();
    key(cellInput(view, 1, 0), "Enter", { metaKey: true });
    await Promise.resolve();
    expect(coords()).toBe("2,0");
    expect(cellInput(view, 3, 0).value).toBe("3");
  });

  it("↓ / ↑ 在首末行之间移动同一列，越过末行 / 表头离开表格，不新增行", async () => {
    const view = mount(twoRows);
    cellInput(view, 0, 1).focus();
    key(cellInput(view, 0, 1), "ArrowDown");
    await Promise.resolve();
    expect(coords()).toBe("1,1");
    key(focused(), "ArrowDown");
    await Promise.resolve();
    key(focused(), "ArrowDown");
    expect(document.activeElement).toBe(view.contentDOM);
    expect(view.state.doc.lineAt(view.state.selection.main.head).text).toBe("后文");
    expect(view.state.doc.toString()).toBe(twoRows);
    cellInput(view, 0, 0).focus();
    key(cellInput(view, 0, 0), "ArrowUp");
    expect(view.state.doc.lineAt(view.state.selection.main.head).number).toBe(2);
  });

  it("← / → 在文字边缘跨格，→ 进下一格时光标在格首", async () => {
    const view = mount(twoRows);
    const first = cellInput(view, 1, 0);
    first.focus();
    first.setSelectionRange(1, 1);
    key(first, "ArrowRight");
    await Promise.resolve();
    expect(coords()).toBe("1,1");
    expect(focused().selectionStart).toBe(0);
    key(focused(), "ArrowLeft");
    await Promise.resolve();
    expect(coords()).toBe("1,0");
    expect(focused().selectionStart).toBe(1);
  });

  it("网格态：Esc 选中整格，方向键换格仍在网格态，Enter 回到编辑，Delete 清空这一格，再按 Esc 离开", async () => {
    const view = mount(twoRows);
    const cell = cellInput(view, 1, 0);
    cell.focus();
    key(cell, "Escape");
    expect(cell.dataset.grid).toBe("true");
    expect(cell.closest("td")?.getAttribute("aria-selected")).toBe("true");
    key(cell, "ArrowRight");
    await Promise.resolve();
    expect(coords()).toBe("1,1");
    expect(focused().dataset.grid).toBe("true");
    key(focused(), "Delete");
    expect(view.state.doc.toString()).toBe(twoRows.replace("| 1 | 2 |", "| 1 |  |"));
    key(focused(), "Enter");
    expect(focused().dataset.grid).toBeUndefined();
    key(focused(), "Escape");
    key(focused(), "Escape");
    expect(document.activeElement).toBe(view.contentDOM);
  });

  it("网格态打字或开始组字即回到编辑，字落在格尾", () => {
    const view = mount(twoRows);
    const cell = cellInput(view, 1, 0);
    cell.focus();
    key(cell, "Escape");
    expect(key(cell, "x").defaultPrevented).toBe(false);
    expect(cell.dataset.grid).toBeUndefined();
    expect(cell.selectionStart).toBe(cell.value.length);
    key(cell, "Escape");
    cell.dispatchEvent(new CompositionEvent("compositionstart"));
    expect(cell.dataset.grid).toBeUndefined();
  });

  it("格内 ⌘B 包上 / 去掉加粗，写回源码", () => {
    const view = mount(twoRows);
    const cell = cellInput(view, 1, 0);
    cell.focus();
    cell.setSelectionRange(0, 1);
    key(cell, "b", { metaKey: true });
    expect(view.state.doc.toString()).toBe(twoRows.replace("| 1 | 2 |", "| **1** | 2 |"));
    key(cell, "b", { metaKey: true });
    expect(view.state.doc.toString()).toBe(twoRows);
  });

  it("从正文用 ↓ 进入表格首格、从下方用 ↑ 进入末行", async () => {
    // 直接跑编辑器键位（happy-dom 在 dispatch 中同步派发 selectionchange，会搅乱真实按键路径）。
    const view = mount(twoRows);
    view.dispatch({ selection: { anchor: view.state.doc.line(2).from } });
    expect(runScopeHandlers(view, new KeyboardEvent("keydown", { key: "ArrowDown" }), "editor")).toBe(true);
    await Promise.resolve();
    expect(coords()).toBe("0,0");
    view.dispatch({ selection: { anchor: view.state.doc.line(7).from } });
    expect(runScopeHandlers(view, new KeyboardEvent("keydown", { key: "ArrowUp" }), "editor")).toBe(true);
    await Promise.resolve();
    expect(coords()).toBe("2,0");
    view.dispatch({ selection: { anchor: 0 } });
    expect(runScopeHandlers(view, new KeyboardEvent("keydown", { key: "ArrowDown" }), "editor")).toBe(false);
  });
});

/** T2 结构入口：行列把手、追加窄条、菜单动作都只改受影响的行（ADR-138 ② 2026-10-03 修订）。 */
describe("table T2 structure entries", () => {
  const content = "| A | B |\n| --- | --- |\n| 1 | 2 |\n\n后文";
  const cellInput = (view: EditorView, row: number, col: number) =>
    view.dom.querySelector<HTMLTextAreaElement>(`textarea[data-row="${row}"][data-column="${col}"]`)!;

  it("表格下方不再常驻四个按钮；行把手在每行首格、列把手在表头各格", () => {
    const view = mount(content);
    expect(view.dom.querySelector(".luna-table-preview__actions")).toBeNull();
    expect(view.dom.querySelectorAll(".luna-table-preview__handle--row")).toHaveLength(2);
    expect(view.dom.querySelectorAll(".luna-table-preview__handle--column")).toHaveLength(2);
  });

  it("点把手打开的是这一行 / 这一列的菜单", () => {
    const calls: unknown[] = [];
    openMenu = (_event, target) => { calls.push(target); };
    const view = mount(content);
    view.dom.querySelectorAll<HTMLButtonElement>(".luna-table-preview__handle--column")[1].click();
    view.dom.querySelectorAll<HTMLButtonElement>(".luna-table-preview__handle--row")[1].click();
    expect(calls).toEqual([
      { tableFrom: 0, row: 0, col: 1, scope: "column" },
      { tableFrom: 0, row: 1, col: 0, scope: "row" },
    ]);
    openMenu = () => {};
  });

  it("追加窄条只在表尾加一行 / 每行末尾加一格", async () => {
    const view = mount(content);
    view.dom.querySelector<HTMLButtonElement>(".luna-table-preview__append--row")!.click();
    expect(view.state.doc.toString()).toBe(content.replace("| 1 | 2 |", "| 1 | 2 |\n|  |  |"));
    await Promise.resolve();
    expect(document.activeElement).toBe(cellInput(view, 2, 0));
    view.dom.querySelector<HTMLButtonElement>(".luna-table-preview__append--column")!.click();
    expect(view.state.doc.toString().split("\n")[0]).toBe("| A | B |  |");
  });

  it("菜单：对齐只改分隔行、插入列落点在新列、表头的删除置灰并说明原因", async () => {
    const view = mount(content);
    const target = { tableFrom: 0, row: 0, col: 0, scope: "cell" as const };
    const items = tableMenuItems({ view, target, labels: LABELS, locale: "en-US", onCopy: () => {} });
    const deleteRow = items.find((item) => item.kind === "action" && item.id === "delete-row");
    expect(deleteRow).toMatchObject({ disabled: true, title: "headerRowFixed" });
    const align = items.find((item) => item.id === "align");
    const center = align?.kind === "submenu" ? align.items.find((item) => item.id === "align-center") : undefined;
    if (center?.kind === "action") center.onSelect();
    expect(view.state.doc.toString()).toBe(content.replace("| --- | --- |", "| :-: | --- |"));
    const insert = tableMenuItems({ view, target, labels: LABELS, locale: "en-US", onCopy: () => {} }).find((item) => item.id === "table-insert");
    const right = insert?.kind === "submenu" ? insert.items.find((item) => item.id === "column-right") : undefined;
    if (right?.kind === "action") right.onSelect();
    await Promise.resolve();
    expect(document.activeElement).toBe(cellInput(view, 0, 1));
    expect(view.state.doc.toString().split("\n")[0]).toBe("| A |  | B |");
  });
});

describe("table incremental update", () => {
  it("在一格里打字只重绘这一格，别的格的渲染节点原样保留", () => {
    const view = mount("| A | B |\n| --- | --- |\n| **1** | 2 |\n| 3 | 4 |\n\n后文");
    const other = view.dom.querySelector('textarea[data-row="2"][data-column="1"]')!.previousElementSibling!.firstChild;
    const input = view.dom.querySelector<HTMLTextAreaElement>('textarea[data-row="1"][data-column="1"]')!;
    input.focus();
    input.value = "22";
    input.dispatchEvent(new Event("input"));
    expect(view.dom.querySelector('textarea[data-row="2"][data-column="1"]')!.previousElementSibling!.firstChild).toBe(other);
    expect(input.previousElementSibling!.textContent).toBe("22");
  });
});

/** T3 区域选择（对标方案 T3）：拖选 / Shift 扩选清空内容，把手选中整行整列删除，下方退格选中整表。 */
describe("table T3 range selection", () => {
  const content = "| A | B | C |\n| --- | --- | --- |\n| 1 | 2 | 3 |\n| 4 | 5 | 6 |\n\n后文";
  const cellInput = (view: EditorView, row: number, col: number) =>
    view.dom.querySelector<HTMLTextAreaElement>(`textarea[data-row="${row}"][data-column="${col}"]`)!;
  const key = (target: HTMLElement, name: string, init: KeyboardEventInit = {}) => {
    const event = new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true, ...init });
    target.dispatchEvent(event);
    return event;
  };
  const selected = (view: EditorView) =>
    [...view.dom.querySelectorAll("[aria-selected='true'] textarea")].map((el) => `${(el as HTMLElement).dataset.row},${(el as HTMLElement).dataset.column}`);

  it("网格态 Shift + 方向键扩成矩形，Delete 一次清空（一步撤销），焦点不动", () => {
    const view = mount(content);
    const start = cellInput(view, 1, 0);
    start.focus();
    key(start, "Escape");
    key(start, "ArrowRight", { shiftKey: true });
    key(start, "ArrowDown", { shiftKey: true });
    expect(selected(view)).toEqual(["1,0", "1,1", "2,0", "2,1"]);
    expect(document.activeElement).toBe(start);
    key(start, "Delete");
    expect(view.state.doc.toString()).toBe(content.replace("| 1 | 2 | 3 |", "|  |  | 3 |").replace("| 4 | 5 | 6 |", "|  |  | 6 |"));
    view.contentDOM.blur();
    expect(undo(view)).toBe(true);
    expect(view.state.doc.toString()).toBe(content);
  });

  it("拖过别的格变成区域选择；Shift + 点击从当前格扩选", () => {
    const view = mount(content);
    const origin = cellInput(view, 1, 1);
    origin.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, buttons: 1 }));
    origin.focus();
    cellInput(view, 2, 2).dispatchEvent(new MouseEvent("mouseenter", { buttons: 1 }));
    expect(selected(view)).toEqual(["1,1", "1,2", "2,1", "2,2"]);
    window.dispatchEvent(new MouseEvent("mouseup"));
    cellInput(view, 0, 0).dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true, shiftKey: true }));
    expect(selected(view)).toEqual(["0,0", "0,1", "1,0", "1,1"]);
  });

  it("多格选区 ⌘C 交出选区写成的表，⌘X 再清空", () => {
    const copies: string[] = [];
    copyRange = (markdown) => { copies.push(markdown); };
    const view = mount(content);
    const start = cellInput(view, 0, 1);
    start.focus();
    key(start, "Escape");
    key(start, "ArrowDown", { shiftKey: true });
    key(start, "c", { metaKey: true });
    expect(copies).toEqual(["| B |\n| --- |\n| 2 |"]);
    key(start, "x", { metaKey: true });
    expect(view.state.doc.toString()).toBe(content.replace("| A | B | C |", "| A |  | C |").replace("| 1 | 2 | 3 |", "| 1 |  | 3 |"));
    copyRange = () => {};
  });

  it("点行把手选中整行，Delete 删掉这一行", () => {
    const view = mount(content);
    view.dom.querySelectorAll<HTMLButtonElement>(".luna-table-preview__handle--row")[1].click();
    expect(selected(view)).toEqual(["1,0", "1,1", "1,2"]);
    key(document.activeElement as HTMLElement, "Delete");
    expect(view.state.doc.toString()).toBe(content.replace("| 1 | 2 | 3 |\n", ""));
  });

  it("⌘A 先选中整张表，再按一次交还正文全选", () => {
    const view = mount(content);
    const start = cellInput(view, 1, 1);
    start.focus();
    key(start, "Escape");
    key(start, "a", { metaKey: true });
    expect(selected(view)).toHaveLength(9);
    // 第二次交给编辑器全选。表格随之切回源码、聚焦中的输入框被移除——happy-dom 会在事务中途
    // 同步派发 selectionchange（浏览器不会），故截下这一笔事务来断言。
    const dispatched: unknown[] = [];
    const spy = vi.spyOn(view, "dispatch").mockImplementation((...specs: unknown[]) => { dispatched.push(...specs); });
    key(start, "a", { metaKey: true });
    spy.mockRestore();
    expect(dispatched).toContainEqual({ selection: { anchor: 0, head: view.state.doc.length } });
  });

  it("表格下方空行按退格先选中整表，再按一次删除整表", async () => {
    const view = mount(content);
    view.dispatch({ selection: { anchor: view.state.doc.line(5).from } });
    expect(runScopeHandlers(view, new KeyboardEvent("keydown", { key: "Backspace" }), "editor")).toBe(true);
    await Promise.resolve();
    expect(selected(view)).toHaveLength(9);
    key(document.activeElement as HTMLElement, "Backspace");
    expect(view.state.doc.toString()).toBe("\n后文");
  });
});

describe("table handle drag", () => {
  const content = "| A | B |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |\n| 5 | 6 |\n\n后文";
  const cellAt = (view: EditorView, row: number, col: number) =>
    view.dom.querySelector(`textarea[data-row="${row}"][data-column="${col}"]`)!.closest("th, td")!;

  it("拖动行把手到第 3 行：这一行移过去（移动不是互换），松手那一下不开菜单", () => {
    const menus: unknown[] = [];
    openMenu = (_event, target) => { menus.push(target); };
    const view = mount(content);
    const handle = view.dom.querySelectorAll<HTMLButtonElement>(".luna-table-preview__handle--row")[1];
    const original = document.elementFromPoint;
    document.elementFromPoint = () => cellAt(view, 3, 0);
    try {
      handle.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, button: 0, clientX: 0, clientY: 0 }));
      window.dispatchEvent(new MouseEvent("pointermove", { clientX: 0, clientY: 40 }));
      expect(cellAt(view, 3, 0).getAttribute("data-drop-target")).toBe("row");
      window.dispatchEvent(new MouseEvent("pointerup"));
      handle.click();
    } finally {
      document.elementFromPoint = original;
      openMenu = () => {};
    }
    expect(view.state.doc.toString()).toBe(content.replace("| 1 | 2 |\n| 3 | 4 |\n| 5 | 6 |", "| 3 | 4 |\n| 5 | 6 |\n| 1 | 2 |"));
    expect(menus).toEqual([]);
  });

  it("位移不到阈值就是点击：照常开菜单，不改正文", () => {
    const menus: unknown[] = [];
    openMenu = (_event, target) => { menus.push(target); };
    const view = mount(content);
    const handle = view.dom.querySelectorAll<HTMLButtonElement>(".luna-table-preview__handle--column")[0];
    handle.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, button: 0, clientX: 0, clientY: 0 }));
    window.dispatchEvent(new MouseEvent("pointermove", { clientX: 1, clientY: 1 }));
    window.dispatchEvent(new MouseEvent("pointerup"));
    handle.click();
    openMenu = () => {};
    expect(menus).toHaveLength(1);
    expect(view.state.doc.toString()).toBe(content);
  });
});

describe("table AI menu entries", () => {
  it("给了 onAi 才列 AI 项；填写这一列带上列号与列名", () => {
    const view = mount("| 产品 | 卖点 |\n| --- | --- |\n| 灯 |  |\n\n后文");
    const target = { tableFrom: 0, row: 0, col: 1, scope: "column" as const };
    expect(tableMenuItems({ view, target, labels: LABELS, locale: "zh-CN", onCopy: () => {} }).some((i) => i.id.startsWith("ai-"))).toBe(false);
    const onAi = vi.fn();
    const items = tableMenuItems({ view, target, labels: LABELS, locale: "zh-CN", onCopy: () => {}, onAi });
    const fill = items.find((i) => i.id === "ai-fill-column");
    if (fill?.kind === "action") fill.onSelect();
    expect(onAi).toHaveBeenCalledWith({ kind: "fill_column", column: 1, header: "卖点" });
  });
});
