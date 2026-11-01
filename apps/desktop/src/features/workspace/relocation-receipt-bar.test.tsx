// @vitest-environment happy-dom
/**
 * 改名 / 移动回执条（PRD-06-030 ③）：说出改了几处、撤销把凭据原样交回、把标签页映射回去。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { createElement } from "react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type { RelocationReceiptDto } from "../../ipc/bindings";
import { announceRelocation } from "../../lib/app-events";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { RelocationReceiptBar, useRelocationReceipt } from "./RelocationReceiptBar";

const RECEIPT: RelocationReceiptDto = {
  links: 3,
  notes: 2,
  undo: [
    {
      from: "a.md",
      to: "b.md",
      attachments: [],
      notes: [{ path: "引用.md", before_hash: "h0", after_hash: "h1" }],
    },
  ],
};

function Host({ onMovedBack, onStatus }: { onMovedBack: (from: string, to: string) => void; onStatus: (m: string) => void }) {
  const [receipt, dismiss] = useRelocationReceipt();
  return receipt
    ? createElement(RelocationReceiptBar, { receipt, onDismiss: dismiss, onMovedBack, onStatus })
    : null;
}

beforeEach(() => resetBindingsDouble());
afterEach(cleanup);

describe("RelocationReceiptBar", () => {
  it("回执说出改了几篇几处；撤销把凭据原样交回并把标签页映射回去", async () => {
    setCommandReply("vaultUndoRelocation", {
      status: "ok",
      data: { moved_back: [{ from: "b.md", to: "a.md" }], restored: ["引用.md"] },
    });
    const onMovedBack = vi.fn();
    const onStatus = vi.fn();
    const view = render(withI18n(createElement(Host, { onMovedBack, onStatus })));
    act(() => announceRelocation(RECEIPT));
    expect(view.getByRole("status").textContent).toContain(t("workspace:relocation.updated", { count: 3, notes: 2 }));

    fireEvent.click(view.getByRole("button", { name: t("workspace:relocation.undo") }));
    await waitFor(() => expect(onMovedBack).toHaveBeenCalledWith("b.md", "a.md"));
    expect(commandMock("vaultUndoRelocation")).toHaveBeenCalledWith(RECEIPT.undo);
    expect(onStatus).toHaveBeenCalledWith(t("workspace:relocation.undone"));
    expect(view.queryByRole("status")).toBeNull();
  });

  it("撤销被拒（之后又改过）：说出原因，标签页不动", async () => {
    setCommandReply("vaultUndoRelocation", { status: "error", error: "VAULT_RELOCATION_STALE: 引用.md" });
    const onMovedBack = vi.fn();
    const onStatus = vi.fn();
    const view = render(withI18n(createElement(Host, { onMovedBack, onStatus })));
    act(() => announceRelocation(RECEIPT));
    fireEvent.click(view.getByRole("button", { name: t("workspace:relocation.undo") }));
    await waitFor(() => expect(onStatus).toHaveBeenCalled());
    expect(onMovedBack).not.toHaveBeenCalled();
    expect(onStatus.mock.calls[0]?.[0]).not.toContain("VAULT_RELOCATION_STALE");
  });

  it("没有可撤销凭据的回执（名字没变）不出条", () => {
    const view = render(withI18n(createElement(Host, { onMovedBack: vi.fn(), onStatus: vi.fn() })));
    act(() => announceRelocation({ links: 0, notes: 0, undo: [] }));
    expect(view.queryByRole("status")).toBeNull();
  });
});
