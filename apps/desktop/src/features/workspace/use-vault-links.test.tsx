// @vitest-environment happy-dom
/**
 * 外部 `lunarca://` 库内容链接（ADR-141，W4a）：被拒说清原因；别的库先问再切（请求留在 Rust，
 * 不清）；拒绝切库就清掉；同库的 open 带行号定位；new 交给确认卡。判定全在 Rust。
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, emitEvent, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { useVaultLinks, type VaultLinkHandlers } from "./use-vault-links";

const flush = () => act(async () => {});

function handlers(overrides: Partial<VaultLinkHandlers> = {}): VaultLinkHandlers {
  return {
    confirmSwitch: vi.fn(async () => true),
    switchVault: vi.fn(),
    open: vi.fn(),
    search: vi.fn(),
    createNamed: vi.fn(async () => {}),
    report: vi.fn(),
    ...overrides,
  };
}

function Host({ h }: { h: VaultLinkHandlers }) {
  useVaultLinks(h);
  return null;
}

const request = (needsSwitch: boolean, action: unknown) => ({
  kind: "request",
  vault_name: "工作库",
  vault_path: "/v/工作库",
  needs_switch: needsSwitch,
  action,
});

afterEach(() => {
  cleanup();
  resetBindingsDouble();
});

describe("useVaultLinks", () => {
  it("挂载时取一次：同库的 open 先清再执行，带上标题行号", async () => {
    setCommandReply("vaultLinkPending", request(false, { kind: "open", path: "a.md", line: 6, heading_index: 1 }));
    const h = handlers();
    render(<Host h={h} />);
    await flush();
    expect(commandMock("vaultLinkDone")).toHaveBeenCalledTimes(1);
    expect(h.open).toHaveBeenCalledWith("a.md", 6, 1);
  });

  it("被拒：说清原因", async () => {
    setCommandReply("vaultLinkPending", { kind: "rejected", reason: "unknown_vault" });
    const h = handlers();
    render(<Host h={h} />);
    await flush();
    expect(h.report).toHaveBeenCalledWith("unknown_vault");
    expect(h.open).not.toHaveBeenCalled();
  });

  it("别的库：先问；同意就切（请求留着等切完），不同意就清掉", async () => {
    setCommandReply("vaultLinkPending", request(true, { kind: "search", query: "季度" }));
    const yes = handlers();
    render(<Host h={yes} />);
    await flush();
    expect(yes.confirmSwitch).toHaveBeenCalledWith("工作库");
    expect(yes.switchVault).toHaveBeenCalledWith("/v/工作库", "工作库");
    expect(commandMock("vaultLinkDone")).not.toHaveBeenCalled();
    expect(yes.search).not.toHaveBeenCalled();
    cleanup();

    const no = handlers({ confirmSwitch: vi.fn(async () => false) });
    render(<Host h={no} />);
    await flush();
    expect(no.switchVault).not.toHaveBeenCalled();
    expect(commandMock("vaultLinkDone")).toHaveBeenCalledTimes(1);
  });

  it("运行中又来一条：随事件再取；new 交给确认卡", async () => {
    setCommandReply("vaultLinkPending", null);
    const h = handlers();
    render(<Host h={h} />);
    await flush();
    expect(h.createNamed).not.toHaveBeenCalled();
    setCommandReply("vaultLinkPending", request(false, { kind: "new", name: "周会" }));
    emitEvent("vaultLinkReceived", {});
    await flush();
    expect(h.createNamed).toHaveBeenCalledWith("周会");
  });
});
