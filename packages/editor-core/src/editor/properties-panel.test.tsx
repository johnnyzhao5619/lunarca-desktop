// @vitest-environment happy-dom
/**
 * 属性面板（ADR-139）：判断与写法全在 Rust（规则端口 `EditorHost.properties`，桌面经 IPC、网页经 WASM），
 * 面板只把编辑交过去、把替换交给编辑器。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { fakeEditorUi, t, withI18n } from "../test-fixture";
import type { PropertyEdit, PropertyResult, PropertyRow, TextChange } from "./editor-host";
import { PropertiesPanel } from "./PropertiesPanel";

const SOURCE = "---\ntitle: 周会\ndone: false\ntags: [工作]\nmeta:\n  a: 1\n---";
let current: string | null = SOURCE;
const applyChange = vi.fn();
const onEditSource = vi.fn();
const focusEditor = vi.fn();
const ROWS: PropertyRow[] = [
  { key: "title", value: { kind: "text", value: "周会" }, locked: null, raw: "周会", line: 2 },
  { key: "done", value: { kind: "checkbox", value: false }, locked: null, raw: "false", line: 3 },
  { key: "tags", value: { kind: "list", value: ["工作"] }, locked: null, raw: "[工作]", line: 4 },
  { key: "meta", value: null, locked: "nested", raw: "a: 1", line: 5 },
];
let editReply: PropertyResult<TextChange> = { ok: true, value: { from: 11, to: 13, insert: "例会" } };
const read = vi.fn<(source: string) => Promise<PropertyResult<PropertyRow[]>>>(async () => ({ ok: true, value: ROWS }));
const edit = vi.fn<(source: string, edit: PropertyEdit) => Promise<PropertyResult<TextChange>>>(async () => editReply);

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});
beforeEach(() => {
  current = SOURCE;
  editReply = { ok: true, value: { from: 11, to: 13, insert: "例会" } };
  [applyChange, onEditSource, focusEditor, read, edit].forEach((fn) => fn.mockClear());
});

function mount() {
  render(
    withI18n(
      createElement(PropertiesPanel, {
        rules: { read, edit },
        ui: fakeEditorUi,
        source: SOURCE,
        currentSource: () => current,
        applyChange,
        onEditSource,
        focusEditor,
      }),
    ),
  );
}

describe("属性面板", () => {
  it("改文本回车提交：编辑交给 Rust，替换交给编辑器", async () => {
    // 钉死 CPU 争用下的事件循环次序：`findBy*` 靠 setTimeout(0) 交还控制，React 的 useEffect 靠
    // setImmediate 冲刷；排程之后晚到 ≥1ms，定时器就先到期，用例在输入框的挂载副作用之前开始打字。
    // 草稿若在副作用里同步外部值，刚打的字会被盖回旧值、回车被当成「没改」——这里每次都走那条路。
    const realSetTimeout = globalThis.setTimeout;
    vi.spyOn(globalThis, "setTimeout").mockImplementation(((run: () => void, ms?: number) => {
      const handle = realSetTimeout(run, ms);
      const until = performance.now() + 2;
      while (!ms && performance.now() < until);
      return handle;
    }) as typeof setTimeout);
    mount();
    const input = await screen.findByLabelText(t("editor.properties.valueLabel", { key: "title" }));
    fireEvent.change(input, { target: { value: "例会" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await vi.waitFor(() =>
      expect(edit).toHaveBeenCalledWith(SOURCE, {
        op: "set",
        key: "title",
        value: { kind: "text", value: "例会" },
      }),
    );
    await vi.waitFor(() => expect(applyChange).toHaveBeenCalledWith({ from: 11, to: 13, insert: "例会" }));
  });

  it("输入法上屏那一下的回车不提交；Esc 撤回草稿并回到正文", async () => {
    mount();
    const input = (await screen.findByLabelText(t("editor.properties.valueLabel", { key: "title" }))) as HTMLInputElement;
    fireEvent.change(input, { target: { value: "zhou" } });
    fireEvent.keyDown(input, { key: "Enter", isComposing: true });
    fireEvent.keyDown(input, { key: "Escape" });
    expect(edit).not.toHaveBeenCalled();
    expect(input.value).toBe("周会");
    expect(focusEditor).toHaveBeenCalled();
  });

  it("等待期间 frontmatter 变了就不落这次替换", async () => {
    mount();
    fireEvent.click(await screen.findByLabelText(t("editor.properties.valueLabel", { key: "done" })));
    current = "---\ntitle: 别的\n---";
    await vi.waitFor(() => expect(edit).toHaveBeenCalled());
    await Promise.resolve();
    expect(applyChange).not.toHaveBeenCalled();
  });

  it("勾选、改类型、列表加项、删属性都是一次编辑", async () => {
    mount();
    fireEvent.click(await screen.findByLabelText(t("editor.properties.valueLabel", { key: "done" })));
    fireEvent.change(screen.getByLabelText(t("editor.properties.typeLabel", { key: "title" })), {
      target: { value: "date" },
    });
    const addItem = screen.getByLabelText(t("editor.properties.addItemTo", { key: "tags" })) as HTMLInputElement;
    fireEvent.change(addItem, { target: { value: "重要" } });
    fireEvent.keyDown(addItem, { key: "Enter" });
    fireEvent.click(screen.getByLabelText(t("editor.properties.remove", { key: "title" })));
    await vi.waitFor(() => expect(edit).toHaveBeenCalledTimes(4));
    const edits = edit.mock.calls.map((call) => call[1]);
    expect(edits[0]).toEqual({ op: "set", key: "done", value: { kind: "checkbox", value: true } });
    expect(edits[1]).toMatchObject({ op: "retype", key: "title", kind: "date", today: expect.stringMatching(/^\d{4}-\d{2}-\d{2}$/u) });
    expect(edits[2]).toEqual({ op: "set", key: "tags", value: { kind: "list", value: ["工作", "重要"] } });
    expect(edits[3]).toEqual({ op: "remove", key: "title" });
    expect(addItem.value).toBe("");
  });

  it("读各行交给规则端口（原文原样给）", async () => {
    mount();
    await screen.findByLabelText(t("editor.properties.valueLabel", { key: "title" }));
    expect(read).toHaveBeenCalledWith(SOURCE);
  });

  it("整段认不出（开头 --- 没闭合）时如实说原因", async () => {
    read.mockResolvedValueOnce({ ok: false, skip: "unclosed" });
    mount();
    expect(await screen.findByText(t("editor.properties.skip.unclosed"))).toBeTruthy();
  });

  it("认不出的写法只读显示原文，给「在源码中编辑」", async () => {
    mount();
    expect(await screen.findByText("a: 1")).toBeTruthy();
    fireEvent.click(screen.getByText(t("editor.properties.editSource")));
    expect(onEditSource).toHaveBeenCalledWith(5);
  });

  it("加属性：回车加一次（卸载时的失焦不再加）；Rust 拒绝时如实说原因", async () => {
    editReply = { ok: false, skip: "key_exists" };
    mount();
    fireEvent.click(await screen.findByText(t("editor.properties.add")));
    const input = screen.getByLabelText(t("editor.properties.newKeyPlaceholder"));
    fireEvent.change(input, { target: { value: "Title" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(await screen.findByText(t("editor.properties.skip.key_exists"))).toBeTruthy();
    expect(edit).toHaveBeenCalledTimes(1);
    expect(edit).toHaveBeenCalledWith(SOURCE, {
      op: "add",
      key: "Title",
      value: { kind: "text", value: "" },
    });
  });
});
