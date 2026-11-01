// @vitest-environment happy-dom
/**
 * 「这一篇的标签」就地增删（ADR-135）：写不写、为什么不能删由 Rust 判，面板只转述。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { NoteTagsPanel } from "./NoteTagsPanel";

afterEach(cleanup);
beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("noteFacts", {
    status: "ok",
    data: {
      note_type: "note",
      draft: false,
      source_material: false,
      created: null,
      modified_ms: null,
      tags: ["项目", "网页端"],
      removable_tags: ["项目"],
    },
  });
});

function mount() {
  render(withI18n(createElement(NoteTagsPanel, { path: "Notes/周报.md", onOpenTag: vi.fn() })));
}

describe("NoteTagsPanel 就地增删", () => {
  it("添加交给 Rust，写成后清空输入框", async () => {
    setCommandReply("editNoteTag", { status: "ok", data: { status: "written" } });
    mount();
    const input = await screen.findByLabelText(t("workspace:panel.noteTagsAddPlaceholder"));
    fireEvent.change(input, { target: { value: "阅读" } });
    fireEvent.click(screen.getByText(t("workspace:panel.noteTagsAdd")));
    await vi.waitFor(() =>
      expect(commandMock("editNoteTag")).toHaveBeenCalledWith("Notes/周报.md", "阅读", true),
    );
    await vi.waitFor(() => expect((input as HTMLInputElement).value).toBe(""));
  });

  it("不在笔记开头的标签：× 的说明不同，点了如实说出口而不改正文", async () => {
    setCommandReply("editNoteTag", { status: "ok", data: { status: "ai_only" } });
    mount();
    expect(
      await screen.findByLabelText(t("workspace:panel.noteTagsRemove", { tag: "项目" })),
    ).toBeTruthy();
    fireEvent.click(
      screen.getByLabelText(t("workspace:panel.noteTagsRemoveElsewhere", { tag: "网页端" })),
    );
    expect(
      await screen.findByText(t("workspace:panel.noteTagsAiOnly", { tag: "网页端" })),
    ).toBeTruthy();
    expect(commandMock("editNoteTag")).toHaveBeenCalledWith("Notes/周报.md", "网页端", false);
  });
});
