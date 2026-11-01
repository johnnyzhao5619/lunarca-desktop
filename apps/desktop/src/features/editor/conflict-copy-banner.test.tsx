// @vitest-environment happy-dom
/**
 * 冲突副本横幅（TDD-06 §4.1 ①，V3b）：说清是谁的副本、从哪来；对比用编辑器里此刻的正文；
 * 保留哪份先确认再交给 Rust，完成后切到原文；git 冲突标记只提示、不替用户选。
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { ConfirmHost } from "../../components/ConfirmHost";
import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { ConflictCopyBanner } from "./ConflictCopyBanner";

const ok = (data: unknown) => ({ status: "ok", data });
const flush = () => act(async () => {});
const COPY = "项目/周报 (Pat's conflicted copy 2026-10-05).md";

afterEach(() => {
  cleanup();
  resetBindingsDouble();
});

function mount(onOpenPath = vi.fn()) {
  render(
    withI18n(
      <ConfirmHost>
        <ConflictCopyBanner path={COPY} currentText={() => "副本正文"} onOpenPath={onOpenPath} onStatus={() => {}} />
      </ConfirmHost>,
    ),
  );
  return onOpenPath;
}

describe("ConflictCopyBanner", () => {
  it("说清是谁的副本；对比用此刻的正文；保留这份先确认再切到原文", async () => {
    setCommandReply("conflictCopyInfo", ok({ path: COPY, kind: "dropbox", original: "项目/周报.md" }));
    setCommandReply("readNote", ok({ content: "原文正文" }));
    setCommandReply("noteEditDiff", ok({ lines: [] }));
    setCommandReply("resolveConflictCopy", ok("项目/周报.md"));
    const onOpenPath = mount();
    await flush();
    expect(screen.getByRole("status").textContent).toContain("这是「周报」的冲突副本（Dropbox 产生）");

    fireEvent.click(screen.getByRole("button", { name: "与原文对比" }));
    await flush();
    expect(commandMock("noteEditDiff")).toHaveBeenCalledWith("原文正文", "副本正文");

    fireEvent.click(screen.getByRole("button", { name: "保留这份" }));
    await flush();
    expect(commandMock("resolveConflictCopy")).not.toHaveBeenCalled();
    const keepButtons = screen.getAllByRole("button", { name: "保留这份" });
    const confirm = keepButtons[keepButtons.length - 1];
    fireEvent.click(confirm);
    await flush();
    expect(commandMock("resolveConflictCopy")).toHaveBeenCalledWith(COPY, true);
    expect(onOpenPath).toHaveBeenCalledWith("项目/周报.md");
  });

  it("git 冲突标记只提示，不给保留按钮", async () => {
    setCommandReply("conflictCopyInfo", ok({ path: "笔记.md", kind: "git_markers", original: null }));
    mount();
    await flush();
    expect(screen.getByRole("status").textContent).toContain("未解决的 git 合并冲突标记");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("不是冲突副本就什么都不显示", async () => {
    setCommandReply("conflictCopyInfo", ok(null));
    mount();
    await flush();
    expect(screen.queryByRole("status")).toBeNull();
  });
});
