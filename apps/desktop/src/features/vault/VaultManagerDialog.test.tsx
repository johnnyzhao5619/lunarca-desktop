// @vitest-environment happy-dom
/**
 * 「管理库」移除与云同步的联动（ADR-151 修订 2026-10-15）。
 *
 * 钉三件事：
 * 1. 正在同步的库：先确认「移除后这台设备将停止同步它；云端副本保留」，取消就什么都不发；
 * 2. 连在另一个账户下的库：确认里写出那个账户，说明这份连接一并解除；
 * 3. 没有同步的库照旧一点即移除；断开失败（钥匙串）按稳定码说清。
 * 移除会不会断开、断开什么全由 Rust 判（`forgetVaultPreflight` / `forgetVault`）。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import {
  commandMock,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";
import type { VaultEntryDto } from "../../ipc/bindings";
import { ConfirmHost } from "../../components/ConfirmHost";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { VaultManagerDialog } from "./VaultManagerDialog";

const ENTRY: VaultEntryDto = {
  path: "/v/read",
  name: "读书",
  active: false,
  missing: false,
  last_opened_ms: 0,
  section: "device",
};

function mount() {
  return render(
    withI18n(
      <ConfirmHost>
        <VaultManagerDialog open onClose={() => {}} onOpenVault={() => {}} />
      </ConfirmHost>,
    ),
  );
}

function confirmDialog(): HTMLElement | null {
  return document.body.querySelector<HTMLElement>("[role=alertdialog]");
}

function lastButton(dialog: HTMLElement): HTMLButtonElement {
  const buttons = dialog.querySelectorAll<HTMLButtonElement>("button");
  return buttons[buttons.length - 1]!;
}

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("listVaults", {
    status: "ok",
    data: { account: null, cloud: false, vaults: [ENTRY] },
  });
  setCommandReply("forgetVault", { status: "ok", data: null });
});
afterEach(cleanup);

describe("管理库：移除与云同步", () => {
  it("正在同步的库：确认写明停止同步、云端副本保留；确认了才移除", async () => {
    setCommandReply("forgetVaultPreflight", { status: "ok", data: { kind: "syncing" } });
    const view = mount();
    fireEvent.click(await view.findByText(t("workspace:vault.forget")));
    await waitFor(() => expect(confirmDialog()).not.toBeNull());
    const dialog = confirmDialog()!;
    expect(dialog.textContent).toContain(t("workspace:vault.forgetSyncTitle", { name: "读书" }));
    expect(dialog.textContent).toContain(t("workspace:vault.forgetSyncing"));
    expect(t("workspace:vault.forgetSyncing")).toContain(
      "这个库正在同步，移除后这台设备将停止同步它；云端副本保留",
    );
    expect(commandMock("forgetVault")).not.toHaveBeenCalled();
    fireEvent.click(lastButton(dialog));
    await waitFor(() => expect(commandMock("forgetVault")).toHaveBeenCalledWith("/v/read"));
  });

  it("取消确认：什么都不发", async () => {
    setCommandReply("forgetVaultPreflight", { status: "ok", data: { kind: "syncing" } });
    const view = mount();
    fireEvent.click(await view.findByText(t("workspace:vault.forget")));
    await waitFor(() => expect(confirmDialog()).not.toBeNull());
    fireEvent.click(confirmDialog()!.querySelector<HTMLButtonElement>("button")!);
    await waitFor(() => expect(confirmDialog()).toBeNull());
    expect(commandMock("forgetVault")).not.toHaveBeenCalled();
  });

  it("连在另一个账户下的库：写出那个账户，说明连接一并解除", async () => {
    setCommandReply("forgetVaultPreflight", {
      status: "ok",
      data: {
        kind: "linked",
        account: { label: "jo***@gmail.com", server_host: "api.lunarca.app", server_official: true },
      },
    });
    const view = mount();
    fireEvent.click(await view.findByText(t("workspace:vault.forget")));
    await waitFor(() => expect(confirmDialog()).not.toBeNull());
    expect(confirmDialog()!.textContent).toContain(
      t("workspace:vault.forgetLinked", { account: "jo***@gmail.com", server: t("cloud:signin.official") }),
    );
    fireEvent.click(lastButton(confirmDialog()!));
    await waitFor(() => expect(commandMock("forgetVault")).toHaveBeenCalledWith("/v/read"));
  });

  it("没有同步的库：不问，直接移除", async () => {
    setCommandReply("forgetVaultPreflight", { status: "ok", data: { kind: "none" } });
    const view = mount();
    fireEvent.click(await view.findByText(t("workspace:vault.forget")));
    await waitFor(() => expect(commandMock("forgetVault")).toHaveBeenCalledWith("/v/read"));
    expect(confirmDialog()).toBeNull();
  });

  it("断开失败：按稳定码说清（记录没删）", async () => {
    setCommandReply("forgetVaultPreflight", { status: "ok", data: { kind: "none" } });
    setCommandReply("forgetVault", { status: "error", error: "sync_keychain" });
    const view = mount();
    fireEvent.click(await view.findByText(t("workspace:vault.forget")));
    expect(await view.findByText(t("cloud:sync.error.sync_keychain"))).toBeTruthy();
  });
});
