// @vitest-environment happy-dom
/**
 * 整库时间点恢复对话框（PRD-13-007）：选时刻 → 预览那一刻 → 恢复到新文件夹 → 可并排打开；
 * 超出保留期说清楚、不让提交。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import {
  commandMock,
  emitEvent,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { formatDateTime } from "../../lib/format-datetime";
import { SyncRestoreDialog } from "./SyncRestoreDialog";

const DEST = "/Users/me/Documents/I2X Next（恢复到 2026-10-12 1530）";

function mount(onClose = vi.fn()) {
  const view = render(withI18n(<SyncRestoreDialog vaultPath="/v/a" vaultName="I2X Next" onClose={onClose} />));
  return { view, onClose };
}

beforeEach(() => resetBindingsDouble());
afterEach(() => cleanup());

describe("SyncRestoreDialog", () => {
  it("预览那一刻、恢复到默认的新文件夹，完成后可并排打开", async () => {
    const committed = Date.UTC(2026, 9, 12, 7, 30);
    setCommandReply("syncRestorePoint", {
      status: "ok",
      data: { at_seq: 42, committed_at_ms: committed, earliest_ms: committed - 86_400_000, destination: DEST },
    });
    setCommandReply("syncRestoreToFolder", {
      status: "ok",
      data: { files: 12, bytes: 2048, skipped: 0, destination: DEST },
    });
    setCommandReply("openVaultWindow", { status: "ok", data: null });
    const { view, onClose } = mount();

    await waitFor(() =>
      expect(view.getByText(DEST)).toBeTruthy(),
    );
    expect(view.container.textContent).toContain(
      t("cloud:sync.restore.state_at", { time: formatDateTime(committed, "zh-CN") }),
    );

    fireEvent.click(view.getByRole("button", { name: t("cloud:sync.restore.submit") }));
    act(() => emitEvent("syncRestoreProgress", { done: 3, total: 12 }));
    await waitFor(() => expect(commandMock("syncRestoreToFolder")).toHaveBeenCalledWith("/v/a", 42, DEST));
    await waitFor(() =>
      expect(view.getByRole("button", { name: t("cloud:sync.restore.open_side_by_side") })).toBeTruthy(),
    );
    fireEvent.click(view.getByRole("button", { name: t("cloud:sync.restore.open_side_by_side") }));
    expect(commandMock("openVaultWindow")).toHaveBeenCalledWith(DEST);
    expect(onClose).toHaveBeenCalled();
  });

  it("换位置只交父文件夹，落点与文件夹名都用 Rust 给回的那一个", async () => {
    const committed = Date.UTC(2026, 9, 12, 7, 30);
    const point = (destination: string) => ({
      status: "ok" as const,
      data: { at_seq: 42, committed_at_ms: committed, earliest_ms: committed - 86_400_000, destination },
    });
    setCommandReply("syncRestorePoint", point(DEST));
    setCommandReply("pickFolder", { status: "ok", data: "/Volumes/Backup" });
    const { view } = mount();
    await waitFor(() => expect(view.getByText(DEST)).toBeTruthy());

    const moved = "/Volumes/Backup/I2X Next（恢复到 2026-10-12 1530）";
    setCommandReply("syncRestorePoint", point(moved));
    fireEvent.click(view.getByRole("button", { name: t("cloud:sync.restore.change_location") }));
    await waitFor(() => expect(view.getByText(moved)).toBeTruthy());
    expect(commandMock("syncRestorePoint")).toHaveBeenLastCalledWith("/v/a", expect.any(Number), "/Volumes/Backup");
  });

  it("超出保留期：说清楚，不能提交", async () => {
    setCommandReply("syncRestorePoint", { status: "error", error: "sync_cursor_expired" });
    const { view } = mount();
    await waitFor(() => expect(view.getByText(t("cloud:sync.restore.expired"))).toBeTruthy());
    const submit = view.getByRole("button", { name: t("cloud:sync.restore.submit") }) as HTMLButtonElement;
    expect(submit.disabled).toBe(true);
  });
});
