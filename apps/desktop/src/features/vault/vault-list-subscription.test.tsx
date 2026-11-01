// @vitest-environment happy-dom
/**
 * 库列表跟着账户走（ADR-151 修订 2026-10-15「续二」）：切换菜单与「管理库」共用一份库清单订阅。
 *
 * 钉四件事：
 * 1. 切换菜单分三节：「本机库」「<账户> 的库」「云端库（不在这台设备上）」——分节由 Rust 给；
 * 2. 点云端库：选文件夹 → `syncCloudVaultDownload` → 切到下载好的库；
 * 3. 真机 2026-10-15 的缺陷：在「管理库」里移除一个库之后，左下角菜单里它还在——Rust 改写注册表
 *    后发 `vaults://changed`，两处必须同时更新（在旧代码上这一例红：切换菜单只在挂载与切库时取）；
 * 4. 换账户：清单换成新账户的（A 的库不再出现，本机库还在）。
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
import type { CloudVaultDto, VaultEntryDto, VaultListDto } from "../../ipc/bindings";
import { ConfirmHost } from "../../components/ConfirmHost";
import { ContextMenuHost } from "../../components/ContextMenuHost";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { VaultManagerDialog } from "./VaultManagerDialog";
import { VaultSwitcher } from "./VaultSwitcher";

function vault(path: string, name: string, overrides: Partial<VaultEntryDto> = {}): VaultEntryDto {
  return {
    path,
    name,
    active: false,
    missing: false,
    last_opened_ms: 0,
    section: "device",
    ...overrides,
  };
}

const DEVICE = vault("/v/local", "本机笔记", { active: true });
const SYNCED = vault("/v/work", "工作", { section: "account" });

function listOf(vaults: VaultEntryDto[], extra: Partial<VaultListDto> = {}): VaultListDto {
  return { account: "jo***@gmail.com", cloud: false, vaults, ...extra };
}

const CLOUD: CloudVaultDto = {
  id: "00000000-0000-0000-0000-0000000000aa",
  name: "旅行",
  confirm_text: "旅行",
  cloud_bytes: 1024,
  last_changed_ms: null,
  place: { kind: "not_here" },
  can_download: true,
  can_delete: true,
  can_restore: false,
};

let currentList: VaultListDto;

function mountSwitcher(onSwitch = vi.fn()) {
  const view = render(
    withI18n(
      <ContextMenuHost>
        <ConfirmHost>
          <VaultSwitcher onManage={() => {}} onSwitch={onSwitch} switchingTo={null} />
          <VaultManagerDialog open onClose={() => {}} onOpenVault={() => {}} />
        </ConfirmHost>
      </ContextMenuHost>,
    ),
  );
  return { view, onSwitch };
}

function trigger(): HTMLButtonElement {
  return document.body.querySelector<HTMLButtonElement>(".luna-vault-switcher__trigger")!;
}

/** 打开切换菜单，返回菜单里每一行（分组标题与条目）的文字。 */
async function openMenu(): Promise<string[]> {
  fireEvent.click(trigger());
  await waitFor(() => expect(document.body.querySelector("[role=menu]")).not.toBeNull());
  const menu = document.body.querySelector("[role=menu]")!;
  return Array.from(
    menu.querySelectorAll<HTMLElement>(".luna-context-menu__heading, .luna-context-menu__label"),
  ).map((node) => node.textContent ?? "");
}

function closeMenu() {
  fireEvent.keyDown(document.body.querySelector("[role=menu]")!, { key: "Escape" });
}

beforeEach(() => {
  resetBindingsDouble();
  currentList = listOf([DEVICE, SYNCED]);
  setCommandReply("listVaults", () => ({ status: "ok", data: currentList }));
  setCommandReply("syncCloudVaults", { status: "ok", data: [CLOUD] });
  setCommandReply("forgetVaultPreflight", { status: "ok", data: { kind: "none" } });
});
afterEach(cleanup);

describe("库切换菜单：分节", () => {
  it("本机库、账户的库、云端库（不在这台设备上）三节，标题与条目都来自 Rust", async () => {
    currentList = listOf([DEVICE, SYNCED], { cloud: true });
    mountSwitcher();
    await waitFor(() => expect(trigger().textContent).toContain("本机笔记"));
    // 云端库一节的数据要等一拍（合并一阵变更再取）。
    await waitFor(() => expect(commandMock("syncCloudVaults")).toHaveBeenCalled());
    await act(async () => {});
    const rows = await openMenu();
    expect(rows).toEqual([
      t("workspace:vault.sectionDevice"),
      "本机笔记",
      t("workspace:vault.sectionAccount", { account: "jo***@gmail.com" }),
      "工作",
      t("workspace:vault.sectionCloud"),
      "旅行",
      t("workspace:vault.manage"),
    ]);
  });

  it("不可列云端库（未登录 / 严格本地）：没有云端库一节，也不去取", async () => {
    currentList = listOf([DEVICE], { account: null, cloud: false });
    mountSwitcher();
    await waitFor(() => expect(trigger().textContent).toContain("本机笔记"));
    const rows = await openMenu();
    expect(rows).toEqual([t("workspace:vault.sectionDevice"), "本机笔记", t("workspace:vault.manage")]);
    expect(commandMock("syncCloudVaults")).not.toHaveBeenCalled();
  });

  it("点云端库：选文件夹、下载到本机，再切到它", async () => {
    currentList = listOf([DEVICE], { cloud: true });
    setCommandReply("pickFolder", { status: "ok", data: "/v/trip" });
    setCommandReply("syncCloudVaultDownload", { status: "ok", data: "/v/trip" });
    const { onSwitch } = mountSwitcher();
    await waitFor(() => expect(commandMock("syncCloudVaults")).toHaveBeenCalled());
    await act(async () => {});
    await openMenu();
    const item = Array.from(document.body.querySelectorAll<HTMLElement>("[role=menuitem]")).find(
      (node) => node.textContent?.includes("旅行"),
    )!;
    fireEvent.click(item);
    await waitFor(() =>
      expect(commandMock("syncCloudVaultDownload")).toHaveBeenCalledWith(CLOUD.id, "/v/trip"),
    );
    await waitFor(() => expect(onSwitch).toHaveBeenCalledWith("/v/trip"));
  });
});

describe("切换菜单与「管理库」共用一份清单", () => {
  it("在「管理库」里移除之后，切换菜单里当场没有它（真机 2026-10-15 缺陷）", async () => {
    // Rust 的行为：改写注册表、广播 `vaults://changed`，之后 `list_vaults` 不再有它。
    setCommandReply("forgetVault", () => {
      currentList = listOf([DEVICE]);
      emitEvent("vaultsChanged", {});
      return { status: "ok", data: null };
    });
    const view = mountSwitcher().view;
    await waitFor(() => expect(trigger().textContent).toContain("本机笔记"));
    expect(await openMenu()).toContain("工作");
    closeMenu();

    const row = Array.from(view.container.querySelectorAll<HTMLElement>(".luna-vault-manager__row")).find(
      (node) => node.textContent?.includes("工作"),
    )!;
    const forget = Array.from(row.querySelectorAll("button")).find(
      (button) => button.textContent === t("workspace:vault.forget"),
    )!;
    fireEvent.click(forget);
    await waitFor(() => expect(commandMock("forgetVault")).toHaveBeenCalledWith("/v/work"));
    await waitFor(() =>
      expect(view.container.querySelector(".luna-vault-manager__list")?.textContent).not.toContain("工作"),
    );
    const rows = await openMenu();
    expect(rows).not.toContain("工作");
    expect(rows).toContain("本机笔记");
  });

  it("换账户：两处都换成新账户的清单——A 的库不见了，本机库还在", async () => {
    const view = mountSwitcher().view;
    await waitFor(() =>
      expect(view.container.querySelector(".luna-vault-manager__list")?.textContent).toContain("工作"),
    );
    currentList = listOf([DEVICE, vault("/v/b", "B 的库", { section: "account" })], {
      account: "b***@x.org",
    });
    act(() => emitEvent("vaultsChanged", {}));
    await waitFor(() =>
      expect(view.container.querySelector(".luna-vault-manager__list")?.textContent).toContain("B 的库"),
    );
    const list = view.container.querySelector(".luna-vault-manager__list")!.textContent ?? "";
    expect(list).not.toContain("工作");
    expect(list).toContain("本机笔记");
    const rows = await openMenu();
    expect(rows).toContain(t("workspace:vault.sectionAccount", { account: "b***@x.org" }));
    expect(rows).not.toContain("工作");
  });
});
