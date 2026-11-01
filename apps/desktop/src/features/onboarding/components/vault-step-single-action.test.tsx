// @vitest-environment happy-dom
/**
 * Vault 那一步只有**一个**动作（PRD-14-040 / 决策 2026-09-11）。
 *
 * 它是引导唯一的必选项，而此前摆着「打开已有文件夹」与「在此新建 Vault」两颗同等权重的
 * 按钮，要求刚装好应用的人先替机器答一个机器自己答得出的问题——`resolve_vault_root` 的
 * `if !create || root.exists()` 意味着 `create_vault_root` 对已存在的目录就是打开。
 *
 * 判据落在渲染与调用上：一颗主按钮、点下去走 `create_vault_root`、`select_vault` 零调用。
 * 反向验证：把那两颗按钮加回去，第一条当场红；把调用换回 `selectVault`，第三条当场红。
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../../ipc/bindings", () => import("../../../ipc/bindings-test-double"));

import {
  commandMock,
  resetBindingsDouble,
  setCommandReply,
} from "../../../ipc/bindings-test-double";
import { withI18n, t } from "../../../i18n/i18n-test-fixture";
import { VaultStep } from "./VaultStep";

afterEach(() => {
  cleanup();
  resetBindingsDouble();
});

const SUGGESTED = "/Users/someone/Documents/LunaVault";

function mount(onBound = () => {}, suggested = { path: SUGGESTED, avoided_onedrive: false }) {
  setCommandReply("vaultStatus", { open: false, root: null });
  setCommandReply("defaultVaultPath", { status: "ok", data: suggested });
  render(withI18n(<VaultStep onBound={onBound} />));
}

describe("Vault 一次点击", () => {
  it("只有一颗主按钮，没有「新建 / 打开」二选一", async () => {
    mount();
    await waitFor(() => {
      expect(screen.getByText(t("onboarding:step.vault.use"))).toBeTruthy();
    });
    // 主按钮 + 换文件夹，就这两个；再多一颗就是把那个二选一又摆回来了。
    const buttons = screen.getAllByRole("button");
    expect(buttons.map((b) => b.textContent?.trim())).toEqual([
      t("onboarding:step.vault.browse"),
      t("onboarding:step.vault.use"),
    ]);
  });

  it("点下去走 create_vault_root——「是新建还是打开」由 Rust 按目录在不在判", async () => {
    const onBound = vi.fn();
    mount(onBound);
    await waitFor(() => {
      expect(screen.getByText(t("onboarding:step.vault.use"))).toBeTruthy();
    });

    fireEvent.click(screen.getByText(t("onboarding:step.vault.use")));

    await waitFor(() => {
      expect(commandMock("createVaultRoot")).toHaveBeenCalledWith(SUGGESTED);
    });
    expect(commandMock("selectVault")).not.toHaveBeenCalled();
    await waitFor(() => {
      expect(onBound).toHaveBeenCalled();
    });
  });
});

/**
 * OneDrive（W1）：「文档」被 OneDrive 重定向时默认位置改到了用户目录下，预填值没改过就说明原因；
 * 输入框里的位置在 OneDrive 里只提示、不拦。判定都在 Rust，这里只看 Rust 说了什么就显示什么。
 */
describe("OneDrive 位置提示", () => {
  const MOVED = "/Users/someone/Lunarca";

  it("默认位置因 OneDrive 改过：说明原因；换了位置就不再说", async () => {
    setCommandReply("vaultPathInOnedrive", false);
    mount(() => {}, { path: MOVED, avoided_onedrive: true });
    await waitFor(() => {
      expect(screen.getByText(t("onboarding:step.vault.onedrive_moved_hint"))).toBeTruthy();
    });
    fireEvent.change(screen.getByLabelText(t("onboarding:step.vault.path_label")), {
      target: { value: "/Volumes/Notes" },
    });
    await waitFor(() => {
      expect(screen.queryByText(t("onboarding:step.vault.onedrive_moved_hint"))).toBeNull();
    });
  });

  it("位置在 OneDrive 里：提示双重同步，但主按钮照常可点", async () => {
    setCommandReply("vaultPathInOnedrive", (path: unknown) => path === SUGGESTED);
    mount();
    await waitFor(() => {
      expect(screen.getByText(t("onboarding:step.vault.onedrive_inside_hint"))).toBeTruthy();
    });
    expect(screen.queryByText(t("onboarding:step.vault.onedrive_moved_hint"))).toBeNull();
    fireEvent.click(screen.getByText(t("onboarding:step.vault.use")));
    await waitFor(() => {
      expect(commandMock("createVaultRoot")).toHaveBeenCalledWith(SUGGESTED);
    });
  });
});
