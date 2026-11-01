// @vitest-environment happy-dom
/**
 * 全局文字速记面板的行为守护（PRD-06-058 B″）。
 *
 * 这扇窗的三件事都只在真实渲染下可见，源码字符串断言一条都证明不了：
 *
 * - 显示桥真的把焦点交回来：面板是预热出来的，组件早在用户第一次按热键之前就挂载完了
 *   ——`autoFocus` 一次都不会再触发。桥断了不报错，表现是"窗浮出来了但打不进字"。
 * - 落盘只发一条命令、且恒带 `drafts`：目的地一旦被前端按情况挑，业务判定就搬进了
 *   界面层（ADR-003）；而两个入口从此可以各自漂。
 * - 不可用时一条命令都不发：给一个按下去只会失败的输入框，比不给更糟。
 *
 * 键盘契约本身不在这里重复测：它归 `CaptureInput`，由 `capture-ux.test.ts` 覆盖一次。
 * 在这里再抄一遍用例，等于承认这两处可以有两套行为——而抽取那个组件就是为了否掉这件事。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

// HUD 外壳借 Tauri 的窗口 API 做拖动与自动增高；两者都与本文件要证明的事无关，替掉即可。
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    setSize: () => Promise.resolve(),
    startDragging: () => Promise.resolve(),
  }),
  LogicalSize: class {
    constructor(
      public width: number,
      public height: number,
    ) {}
  },
}));

import {
  commandMock,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { TextQuickNoteHud } from "./TextQuickNoteHud";
import { NAVIGATE_EVENT } from "../../lib/navigate";

/** 可用性快照：`text_quick_note` 那一行的状态由用例给。 */
function snapshot(state: unknown) {
  return {
    status: "ok",
    data: { revision: 1, actions: [{ id: "text_quick_note", state }] },
  };
}

const AVAILABLE = { kind: "available" };

function mount() {
  const view = render(withI18n(<TextQuickNoteHud />));
  return { view, input: () => view.container.querySelector("textarea") };
}

/** 本轮里应用要求去的地方（`lib/navigate.ts` 在这类窗口里派发的那一串）。 */
const navigations: string[] = [];
const recordNavigation = (event: Event) => {
  navigations.push((event as CustomEvent<string>).detail);
};

beforeEach(() => {
  navigations.length = 0;
  window.addEventListener(NAVIGATE_EVENT, recordNavigation);
  resetBindingsDouble();
  setCommandReply("createActionStates", snapshot(AVAILABLE));
  setCommandReply("createNoteFromText", {
    status: "ok",
    data: { path: "QuickNotes/2026-09-09 1330 一条速记.md" },
  });
});

afterEach(() => {
  window.removeEventListener(NAVIGATE_EVENT, recordNavigation);
  cleanup();
});

describe("全局文字速记面板（PRD-06-058 B″）", () => {
  it("显示桥把焦点交回输入框——面板是预热的，`autoFocus` 不会再触发", async () => {
    const { input } = mount();
    await waitFor(() => expect(input()).not.toBeNull());
    // 先把焦点挪走，模拟"上一次用完之后窗被隐藏、焦点早就不在这儿了"。
    input()!.blur();
    expect(document.activeElement).not.toBe(input());
    window.__lunaShowQuickNote!("shortcut");
    expect(document.activeElement, "桥断了的表现是窗浮出来但打不进字").toBe(input());
  });

  it("落盘只有一条命令，且目的地恒为 `drafts`（判定在 Rust）", async () => {
    const { input } = mount();
    await waitFor(() => expect(input()).not.toBeNull());
    fireEvent.change(input()!, { target: { value: "全局面板里记的一条" } });
    fireEvent.keyDown(input()!, { key: "Enter" });
    await waitFor(() =>
      expect(commandMock("createNoteFromText")).toHaveBeenCalledWith(
        "全局面板里记的一条",
        "drafts",
      ),
    );
    // 回车不把主窗拉到前台：那是 ⌘回车的语义，混起来会让连续捕获每存一条就跳一次窗。
    expect(navigations).toEqual([]);
  });

  it("⌘/Ctrl+回车 = 落盘后把主窗带到前台并选中该篇，然后收起面板", async () => {
    const { input } = mount();
    await waitFor(() => expect(input()).not.toBeNull());
    fireEvent.change(input()!, { target: { value: "要展开细写的这条" } });
    fireEvent.keyDown(input()!, { key: "Enter", metaKey: true });
    // 断言的是**去了哪儿**：这扇窗在真机上会走「唤起主窗」那条通道，而通道选择只有一处
    // 实现（`lib/navigate.ts`），也只在那里有一条用例。
    await waitFor(() =>
      expect(navigations).toEqual(["quick-notes://QuickNotes/2026-09-09 1330 一条速记.md"]),
    );
    expect(commandMock("textQuickNoteClose")).toHaveBeenCalled();
  });

  it("库没打开时保留输入框但禁用提交，也一条落盘命令都不发", async () => {
    setCommandReply(
      "createActionStates",
      snapshot({ kind: "unavailable", reason: "vault_unavailable" }),
    );
    const { view, input } = mount();
    await waitFor(() =>
      // 原因文案来自创建动作注册表——同一个原因在「添加内容」与这里不该有两种说法。
      expect(view.container.querySelector('.luna-hud-receipt[data-kind="error"]')?.textContent).toBe(
        t("workspace:create.reason.vaultUnavailable"),
      ),
    );
    expect(input()).not.toBeNull();
    expect(input()!.disabled).toBe(true);
    expect(commandMock("createNoteFromText")).not.toHaveBeenCalled();
  });

  it("落盘失败不清空——用户刚敲的那段字是他仅有的一份", async () => {
    setCommandReply("createNoteFromText", { status: "error", error: "VAULT_NOT_OPEN" });
    const { view, input } = mount();
    await waitFor(() => expect(input()).not.toBeNull());
    fireEvent.change(input()!, { target: { value: "写了一半" } });
    fireEvent.keyDown(input()!, { key: "Enter" });
    await waitFor(() => expect(commandMock("createNoteFromText")).toHaveBeenCalled());
    expect(input()!.value).toBe("写了一半");
    expect(view.getByRole("alert").textContent).toContain("内容仍保留");
  });

  it("入口面按唤起来源归档：托盘不混记成快捷键", async () => {
    const { input } = mount();
    await waitFor(() => expect(input()).not.toBeNull());
    window.__lunaShowQuickNote!("tray");
    fireEvent.change(input()!, { target: { value: "从托盘进来的这条" } });
    fireEvent.keyDown(input()!, { key: "Enter" });
    await waitFor(() => expect(commandMock("recordWorkspaceMetric")).toHaveBeenCalled());
    const calls = commandMock("recordWorkspaceMetric").mock.calls as Array<
      [Record<string, unknown>]
    >;
    // 两条路径的到达率必须分得开——混成同一个数就等于没测（PRD-07-070）。
    expect(calls[calls.length - 1][0]).toMatchObject({
      kind: "create_entry_finished",
      action: "text_quick_note",
      entry: "tray",
      result: "arrived",
    });
  });

  it("存下的回执在行内、带「打开」，敲下一条时让位", async () => {
    setCommandReply("createNoteFromText", { status: "ok", data: { path: "QuickNotes/一条.md" } });
    const { view, input } = mount();
    await waitFor(() => expect(input()).not.toBeNull());
    fireEvent.change(input()!, { target: { value: "第一条" } });
    fireEvent.keyDown(input()!, { key: "Enter" });
    const receipt = await view.findByRole("status");
    expect(receipt.textContent).toContain(t("workspace:capture.hud.savedCount", { count: 1 }));
    expect(receipt.closest(".luna-hud-header")).not.toBeNull();
    fireEvent.change(input()!, { target: { value: "第" } });
    await waitFor(() => expect(view.container.querySelector('.luna-hud-receipt[data-kind="success"]')).toBeNull());
  });
});
