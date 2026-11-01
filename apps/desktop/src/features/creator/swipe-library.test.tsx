// @vitest-environment happy-dom
/**
 * 灵感库页（ADR-162 ⑬，PRD-19-005）：分组来自 Rust；归类提案先改后写，写回的入参是改过的草稿。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type { SwipeItemDto } from "../../ipc/bindings";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { SwipeLibrary } from "./SwipeLibrary";

afterEach(cleanup);
beforeEach(resetBindingsDouble);

const item = (title: string, why: string | null = null): SwipeItemDto => ({
  path: `Creator/Swipe/${title}.md`,
  title,
  modified_ms: 1,
  why,
});

function renderLibrary(onOpenNote: (path: string) => void = () => {}) {
  setCommandReply("creatorSwipeLibrary", {
    status: "ok",
    data: {
      total: 3,
      groups: [
        { name: "悬念", items: [item("好开头", "先立一个错的常识。")] },
        { name: null, items: [item("反问"), item("清单体")] },
      ],
    },
  });
  return render(
    withI18n(createElement(SwipeLibrary, { onBack: () => {}, onOpenNote, onCreateSwipe: () => {} })),
  );
}

describe("灵感库页", () => {
  it("按套路分组列出，没归类的在「未归类」；点一条打开它", async () => {
    const onOpenNote = vi.fn();
    renderLibrary(onOpenNote);
    const suspense = within(await screen.findByRole("region", { name: "悬念" }));
    expect(suspense.getByText("先立一个错的常识。")).toBeTruthy();
    const loose = within(screen.getByRole("region", { name: t("workspace:creator.swipeLibrary.loose") }));
    fireEvent.click(loose.getByText("反问"));
    expect(onOpenNote).toHaveBeenCalledWith("Creator/Swipe/反问.md");
  });

  it("归类先出提案：改类名、挪出条目后写回的是改过的草稿；写完说清写了几条", async () => {
    setCommandReply("creatorSuggestSwipePatterns", {
      status: "ok",
      data: {
        considered: 3,
        total: 3,
        groups: [
          { name: "悬念", members: [item("好开头"), item("反问")] },
          { name: "清单", members: [item("清单体")] },
        ],
      },
    });
    setCommandReply("creatorApplySwipePatterns", { status: "ok", data: { written: 2, skipped: [] } });
    renderLibrary();
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.swipeLibrary.suggest") }));
    const first = await screen.findByRole("textbox", { name: t("workspace:creator.swipeLibrary.groupName", { index: 1 }) });
    fireEvent.change(first, { target: { value: "反常识" } });
    fireEvent.click(
      screen.getByRole("button", { name: t("workspace:creator.swipeLibrary.removeMember", { title: "反问" }) }),
    );
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.swipeLibrary.apply") }));
    await waitFor(() =>
      expect(commandMock("creatorApplySwipePatterns")).toHaveBeenCalledWith([
        { name: "反常识", paths: ["Creator/Swipe/好开头.md"] },
        { name: "清单", paths: ["Creator/Swipe/清单体.md"] },
      ]),
    );
    expect(await screen.findByText(t("workspace:creator.swipeLibrary.applied", { count: 2 }))).toBeTruthy();
    expect(screen.queryByRole("button", { name: t("workspace:creator.swipeLibrary.apply") })).toBeNull();
  });

  it("归类失败（灵感太少、模型不可用）如实说明，不出提案", async () => {
    setCommandReply("creatorSuggestSwipePatterns", { status: "error", error: "CREATOR_SWIPE_TOO_FEW: 至少要有 3 条灵感才能归类。" });
    renderLibrary();
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.swipeLibrary.suggest") }));
    // 失败就近写在页头下面（ADR-165 ⑨），以 alert 播报。
    expect((await screen.findByRole("alert")).textContent).toContain("至少要有 3 条灵感才能归类。");
    expect(screen.queryByRole("button", { name: t("workspace:creator.swipeLibrary.apply") })).toBeNull();
  });
});
