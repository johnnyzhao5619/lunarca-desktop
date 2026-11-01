/**
 * 新建库 / 打开文件夹为库的 OneDrive 提示（W1）：「文档」在 OneDrive 里时新建库的选择器标题说明
 * 建议位置；选中的位置在 OneDrive 里先问一句，用户说「仍然使用」就照常建，**不拦**。判定都在 Rust
 * （`default_vault_path` / `vault_path_in_onedrive`），这里只看流程有没有按 Rust 的答复走。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { pickAndOpenVault, type VaultPickPrompts } from "./vault-actions";

beforeEach(() => {
  setCommandReply("defaultVaultPath", { status: "ok", data: { path: "/Users/me/Documents/LunaVault", avoided_onedrive: false } });
});

afterEach(() => {
  resetBindingsDouble();
});

const PICKED = "/Users/me/OneDrive/Notes";

function prompts(confirmed: boolean): VaultPickPrompts & { confirmOneDrive: ReturnType<typeof vi.fn> } {
  return {
    oneDriveTitle: (suggested) => `title:${suggested}`,
    confirmOneDrive: vi.fn(() => Promise.resolve(confirmed)),
  };
}

describe("OneDrive 位置提示", () => {
  it("「文档」在 OneDrive 里：新建库的选择器标题带上建议位置", async () => {
    setCommandReply("defaultVaultPath", { status: "ok", data: { path: "/Users/me/Lunarca", avoided_onedrive: true } });
    setCommandReply("pickFolder", { status: "ok", data: null });
    await pickAndOpenVault(true, prompts(true));
    expect(commandMock("pickFolder")).toHaveBeenCalledWith("title:/Users/me/Lunarca");
  });

  it("默认位置没因 OneDrive 改过、或是打开已有文件夹：标题照旧", async () => {
    setCommandReply("defaultVaultPath", { status: "ok", data: { path: "/Users/me/Documents/LunaVault", avoided_onedrive: false } });
    setCommandReply("pickFolder", { status: "ok", data: null });
    await pickAndOpenVault(true, prompts(true));
    await pickAndOpenVault(false, prompts(true));
    expect(commandMock("pickFolder").mock.calls).toEqual([[null], [null]]);
  });

  it("选在 OneDrive 里：先问；用户取消就不建", async () => {
    setCommandReply("pickFolder", { status: "ok", data: PICKED });
    setCommandReply("vaultPathInOnedrive", true);
    const declined = prompts(false);
    expect(await pickAndOpenVault(true, declined)).toEqual({ kind: "cancelled" });
    expect(declined.confirmOneDrive).toHaveBeenCalledOnce();
    expect(commandMock("createVaultRoot")).not.toHaveBeenCalled();
  });

  it("选在 OneDrive 里、用户说仍然使用：照常建（只提示不拦）", async () => {
    setCommandReply("pickFolder", { status: "ok", data: PICKED });
    setCommandReply("vaultPathInOnedrive", true);
    expect(await pickAndOpenVault(false, prompts(true))).toEqual({ kind: "done", path: PICKED });
    expect(commandMock("selectVault")).toHaveBeenCalledWith(PICKED);
  });

  it("不在 OneDrive 里：不问", async () => {
    setCommandReply("pickFolder", { status: "ok", data: "/Users/me/Lunarca" });
    setCommandReply("vaultPathInOnedrive", false);
    const accepted = prompts(true);
    expect(await pickAndOpenVault(true, accepted)).toEqual({ kind: "done", path: "/Users/me/Lunarca" });
    expect(accepted.confirmOneDrive).not.toHaveBeenCalled();
  });
});
