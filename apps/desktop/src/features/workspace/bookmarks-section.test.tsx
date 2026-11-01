// @vitest-environment happy-dom
/**
 * 文件树上方的书签区（PRD-06-006，V5b）：没有书签就不出现；列出 Rust 给的书签（只列还在的，
 * 判定在 Rust）；点开交给外层；可收起。
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { ContextMenuHost } from "../../components/ContextMenuHost";
import { BookmarksSection, useVaultBookmarks } from "./BookmarksSection";

const flush = () => act(async () => {});

function Host({ onOpen }: { onOpen: (path: string) => void }) {
  const bookmarks = useVaultBookmarks();
  return (
    <>
      <BookmarksSection bookmarks={bookmarks} onOpen={(b) => onOpen(b.path)} onStatus={() => {}} />
      <button type="button" data-testid="add" onClick={() => void bookmarks.setBookmarked("新.md", true)} />
    </>
  );
}

afterEach(() => {
  cleanup();
  resetBindingsDouble();
});

describe("BookmarksSection", () => {
  it("没有书签时不出现", async () => {
    setCommandReply("bookmarks", { status: "ok", data: [] });
    render(withI18n(<ContextMenuHost><Host onOpen={() => {}} /></ContextMenuHost>));
    await flush();
    expect(screen.queryByRole("region", { name: "书签" })).toBeNull();
  });

  it("列出书签、点开交给外层、可收起；增删后用 Rust 回的新列表", async () => {
    setCommandReply("bookmarks", {
      status: "ok",
      data: [
        { path: "项目/计划.md", is_dir: false },
        { path: "归档", is_dir: true },
      ],
    });
    setCommandReply("setBookmark", {
      status: "ok",
      data: [
        { path: "项目/计划.md", is_dir: false },
        { path: "归档", is_dir: true },
        { path: "新.md", is_dir: false },
      ],
    });
    const onOpen = vi.fn();
    render(withI18n(<ContextMenuHost><Host onOpen={onOpen} /></ContextMenuHost>));
    await flush();
    fireEvent.click(screen.getByRole("button", { name: "计划" }));
    expect(onOpen).toHaveBeenCalledWith("项目/计划.md");
    fireEvent.click(screen.getByTestId("add"));
    await flush();
    expect(commandMock("setBookmark")).toHaveBeenCalledWith("新.md", true);
    expect(screen.getByRole("button", { name: "新" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /书签/ }));
    expect(screen.queryByRole("button", { name: "计划" })).toBeNull();
  });
});
