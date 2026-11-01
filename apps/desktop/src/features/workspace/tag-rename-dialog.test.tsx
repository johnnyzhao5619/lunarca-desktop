// @vitest-environment happy-dom
/**
 * 标签改名 / 合并对话框（ADR-135）：判定全在 Rust，前端把预览与回执原样带回。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { TagRenameDialog } from "./TagRenameDialog";

afterEach(cleanup);
beforeEach(resetBindingsDouble);

const TARGETS = [
  { path: "Notes/甲.md", before_hash: "h-甲" },
  { path: "Notes/乙.md", before_hash: "h-乙" },
];
const CHANGED = [
  { path: "Notes/甲.md", before_hash: "h-甲", after_hash: "a-甲" },
  { path: "Notes/乙.md", before_hash: "h-乙", after_hash: "a-乙" },
];

function mount(onRenamed = vi.fn()) {
  render(withI18n(createElement(TagRenameDialog, { tag: "项目", onClose: vi.fn(), onRenamed })));
  return onRenamed;
}

function rename(to: string) {
  const input = screen.getByLabelText(t("workspace:tagRename.newName"));
  fireEvent.change(input, { target: { value: to } });
  fireEvent.click(screen.getByText(t("workspace:tagRename.preview")));
}

describe("TagRenameDialog", () => {
  it("名字不合法时就地说明，不进入预览", async () => {
    setCommandReply("previewTagRename", { status: "ok", data: { status: "invalid_name" } });
    mount();
    rename("有 空格");
    expect(await screen.findByText(t("workspace:tagRename.error.invalid_name"))).toBeTruthy();
  });

  it("预览说明合并与跳过原因；确认带回每篇指纹；撤销带回回执", async () => {
    setCommandReply("previewTagRename", {
      status: "ok",
      data: {
        status: "ready",
        from: "项目",
        to: "工作",
        merge: true,
        targets: TARGETS,
        skipped: [{ path: "Notes/丙.md", reason: "unsupported_form" }],
      },
    });
    setCommandReply("applyTagRename", {
      status: "ok",
      data: { changed: CHANGED, stale: [], skipped: [], failed: [] },
    });
    setCommandReply("undoTagRename", {
      status: "ok",
      data: { restored: ["Notes/甲.md"], kept: ["Notes/乙.md"] },
    });
    const onRenamed = mount();
    rename("工作");
    expect(await screen.findByText(t("workspace:tagRename.merge", { to: "工作" }))).toBeTruthy();
    expect(screen.getByText(t("workspace:tagRename.reason.unsupported_form"))).toBeTruthy();

    fireEvent.click(screen.getByText(t("workspace:tagRename.confirmMerge", { count: 2 })));
    expect(await screen.findByText(t("workspace:tagRename.done", { count: 2 }))).toBeTruthy();
    expect(commandMock("applyTagRename")).toHaveBeenCalledWith("项目", "工作", TARGETS);
    expect(onRenamed).toHaveBeenCalledWith("项目", "工作");

    fireEvent.click(screen.getByText(t("workspace:tagRename.undo")));
    expect(await screen.findByText(t("workspace:tagRename.undone", { count: 1 }))).toBeTruthy();
    expect(screen.getByText(t("workspace:tagRename.undoKept", { count: 1 }))).toBeTruthy();
    expect(commandMock("undoTagRename")).toHaveBeenCalledWith(CHANGED);
  });
});
