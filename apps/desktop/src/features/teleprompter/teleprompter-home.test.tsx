// @vitest-environment happy-dom
/**
 * 提词器首页（ADR-176 ②，PRD-20-004 / 005）：最近列表、开始提词、新建提词稿与跟读开关。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { TeleprompterHome } from "./TeleprompterHome";

afterEach(cleanup);

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("listSettings", { status: "ok", data: [] });
  setCommandReply("setSetting", { status: "ok", data: null });
  setCommandReply("teleprompterOpen", { status: "ok", data: null });
  setCommandReply("teleprompterHome", {
    status: "ok",
    data: {
      current: "Notes/口播.md",
      recent: [
        { path: "Notes/口播.md", title: "口播" },
        { path: "Teleprompter/提词稿.md", title: "提词稿" },
      ],
      draft_dir: "Teleprompter",
    },
  });
});

describe("提词器首页", () => {
  it("列出最近提词的稿子；主按钮继续当前那一篇；点一行用提词器打开它", async () => {
    render(withI18n(createElement(TeleprompterHome)));
    const start = await screen.findByRole("button", { name: t("workspace:teleprompter.home.startCurrent", { title: "口播" }) });
    fireEvent.click(start);
    await waitFor(() => expect(commandMock("teleprompterOpen")).toHaveBeenCalledWith("Notes/口播.md", false));
    fireEvent.click(screen.getAllByRole("button", { name: t("workspace:teleprompter.home.open") })[1]);
    await waitFor(() => expect(commandMock("teleprompterOpen")).toHaveBeenCalledWith("Teleprompter/提词稿.md", false));
    expect(screen.getByText(t("workspace:teleprompter.home.draftHint", { dir: "Teleprompter" }))).toBeTruthy();
  });

  it("跟读开关默认关；打开即写设置", async () => {
    render(withI18n(createElement(TeleprompterHome)));
    const toggle = await screen.findByRole("switch", { name: t("workspace:teleprompter.home.follow") });
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(toggle);
    await waitFor(() =>
      expect(commandMock("setSetting")).toHaveBeenCalledWith("teleprompter.follow_enabled", { kind: "bool", value: true }),
    );
  });

  it("从剪贴板新建：Rust 建好即打开；剪贴板没有文字时如实说明", async () => {
    setCommandReply("teleprompterNewDraft", { status: "ok", data: "Teleprompter/提词稿 1.md" });
    render(withI18n(createElement(TeleprompterHome)));
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:teleprompter.home.fromClipboard") }));
    await waitFor(() => expect(commandMock("teleprompterNewDraft")).toHaveBeenCalledWith({ kind: "clipboard" }));

    setCommandReply("teleprompterNewDraft", { status: "error", error: "TELEPROMPTER_CLIPBOARD_EMPTY: 剪贴板里没有文字。" });
    fireEvent.click(screen.getByRole("button", { name: t("workspace:teleprompter.home.fromClipboard") }));
    expect(await screen.findByRole("alert")).toBeTruthy();
  });

  it("还没提词过：空态说明从哪里开始，主按钮不可点", async () => {
    setCommandReply("teleprompterHome", { status: "ok", data: { current: null, recent: [], draft_dir: "Teleprompter" } });
    render(withI18n(createElement(TeleprompterHome)));
    expect(await screen.findByText(t("workspace:teleprompter.home.recentEmpty"))).toBeTruthy();
    expect((screen.getByRole("button", { name: t("workspace:teleprompter.home.start") }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("首页上 ⌘V：粘贴的文字存成提词稿；粘进输入框的不接", async () => {
    setCommandReply("teleprompterNewDraft", { status: "ok", data: "Teleprompter/大家好.md" });
    render(withI18n(createElement(TeleprompterHome)));
    await screen.findByText(t("workspace:teleprompter.home.sendHint"));
    const paste = (target: EventTarget, text: string) => {
      const event = new Event("paste", { bubbles: true, cancelable: true }) as ClipboardEvent;
      Object.defineProperty(event, "clipboardData", { value: { getData: () => text } });
      target.dispatchEvent(event);
    };
    paste(document.body, "大家好，今天聊三件事");
    await waitFor(() =>
      expect(commandMock("teleprompterNewDraft")).toHaveBeenCalledWith({ kind: "text", text: "大家好，今天聊三件事" }),
    );
    const input = document.createElement("input");
    document.body.appendChild(input);
    paste(input, "别的");
    expect(commandMock("teleprompterNewDraft")).toHaveBeenCalledTimes(1);
    input.remove();
  });

  it("文件拖到首页：交给 Rust 转成提词稿", async () => {
    setCommandReply("teleprompterImportFiles", { status: "ok", data: ["Teleprompter/稿子.md"] });
    const { container } = render(withI18n(createElement(TeleprompterHome)));
    await screen.findByText(t("workspace:teleprompter.home.sendHint"));
    const section = container.querySelector("[data-teleprompter-drop]")!;
    section.dispatchEvent(new CustomEvent("luna:teleprompter-file-drop", { detail: { paths: ["/tmp/稿子.docx"] } }));
    await waitFor(() => expect(commandMock("teleprompterImportFiles")).toHaveBeenCalledWith(["/tmp/稿子.docx"]));
  });
});
