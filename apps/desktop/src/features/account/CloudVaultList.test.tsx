// @vitest-environment happy-dom
/**
 * 账户页「云端库」（ADR-151 修订 2026-10-15）。
 *
 * 钉只有真渲染才看得见的几件事：
 * 1. 每行的处境与按钮全照 Rust 的裁决：同步在这里写出本地库、不在这台设备上给「下载到本机」、
 *    已删除的单独一组带清除日期与「恢复」；名字读不出如实写「名字暂不可读」；
 * 2. 删除要照着输入 Rust 给的那段字才放行，确认框写明影响所有设备、本地文件都保留、30 天可恢复；
 *    组织成员删组织库（403）说只有管理员能删；
 * 3. 恢复的两种失败（超容量、已过恢复期）各有一句；
 * 4. 下载到本机：选文件夹 → 命令带上那个文件夹；非空文件夹按稳定码说清。
 */

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { vi } from "vitest";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import {
  commandMock,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";
import type { CloudVaultDto } from "../../ipc/bindings";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { formatDate } from "../../lib/format-datetime";
import { CloudVaultList } from "./CloudVaultList";

const PURGE = Date.UTC(2026, 10, 14);

function cloudVault(overrides: Partial<CloudVaultDto> = {}): CloudVaultDto {
  return {
    id: "00000000-0000-0000-0000-00000000000a",
    name: "工作",
    confirm_text: "工作",
    cloud_bytes: 4096,
    last_changed_ms: Date.UTC(2026, 9, 10, 8, 0, 0),
    place: { kind: "not_here" },
    can_download: true,
    can_delete: true,
    can_restore: false,
    ...overrides,
  };
}

const HERE = cloudVault({
  id: "here",
  name: "读书",
  confirm_text: "读书",
  place: { kind: "here", vault_path: "/v/read", local_name: "读书笔记", display_path: "~/v/read" },
  can_download: false,
});
const AWAY = cloudVault({ id: "away" });
const UNREADABLE = cloudVault({ id: "abcdef12-0000", name: null, confirm_text: "abcdef12" });
const DELETED = cloudVault({
  id: "gone",
  name: "旧库",
  confirm_text: "旧库",
  place: { kind: "deleted", deleted_ms: Date.UTC(2026, 9, 15), purge_after_ms: PURGE },
  can_download: false,
  can_delete: false,
  can_restore: true,
});

function mount() {
  return render(withI18n(<CloudVaultList refreshKey="" />));
}

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("syncCloudVaults", { status: "ok", data: [HERE, AWAY, UNREADABLE, DELETED] });
  for (const name of ["syncCloudVaultDelete", "syncCloudVaultRestore"]) {
    setCommandReply(name, { status: "ok", data: null });
  }
});
afterEach(cleanup);

/** 删除确认框（挂在 body 上）。 */
function deleteDialog(): HTMLElement {
  const dialog = document.body.querySelector<HTMLElement>("[role=alertdialog]");
  if (!dialog) throw new Error("没有删除确认框");
  return dialog;
}

function rowOf(view: ReturnType<typeof mount>, text: string): HTMLElement {
  return view.getByText(text).closest("li") as HTMLElement;
}

describe("云端库", () => {
  it("每行的处境与按钮照 Rust 的裁决；已删除的单独一组带清除日期", async () => {
    const view = mount();
    await view.findByText(t("cloud:sync.cloud_vaults.here", { name: "读书笔记" }));
    const here = rowOf(view, "读书");
    expect(here.textContent).toContain("~/v/read");
    expect(here.textContent).not.toContain(t("cloud:sync.cloud_vaults.download"));
    expect(here.textContent).toContain(t("cloud:sync.cloud_vaults.delete"));

    const away = rowOf(view, "工作");
    expect(away.textContent).toContain(t("cloud:sync.cloud_vaults.not_here"));
    expect(away.textContent).toContain(t("cloud:sync.cloud_vaults.download"));

    // 名字读不出：如实说，不编。
    expect(view.getByText(t("cloud:sync.cloud_vaults.name_unreadable"))).toBeTruthy();

    const deletedGroup = view.getByRole("list", { name: t("cloud:sync.cloud_vaults.deleted_title") });
    expect(deletedGroup.textContent).toContain("旧库");
    expect(deletedGroup.textContent).toContain(
      t("cloud:sync.cloud_vaults.purge_on", { date: formatDate(PURGE, "zh-CN") }),
    );
    expect(deletedGroup.textContent).toContain(t("cloud:sync.cloud_vaults.restore"));
    expect(deletedGroup.textContent).not.toContain(t("cloud:sync.cloud_vaults.delete"));
    const active = view.getByRole("list", { name: t("cloud:sync.cloud_vaults.title") });
    expect(active.textContent).not.toContain("旧库");
  });

  it("过了恢复期：不给恢复，写明即将彻底清除", async () => {
    setCommandReply("syncCloudVaults", {
      status: "ok",
      data: [{ ...DELETED, can_restore: false }],
    });
    const view = mount();
    await view.findByText(t("cloud:sync.cloud_vaults.purge_due"));
    expect(view.queryByText(t("cloud:sync.cloud_vaults.restore"))).toBeNull();
  });

  it("删除要照着输入库名才放行；确认框写明影响所有设备、本地文件都保留、可恢复", async () => {
    const view = mount();
    await view.findByText("工作");
    fireEvent.click(
      rowOf(view, "工作").querySelector<HTMLButtonElement>(".luna-btn--danger")!,
    );
    const dialog = deleteDialog();
    expect(dialog.textContent).toContain(
      t("cloud:sync.cloud_vaults.delete_dialog.title", { name: "工作" }),
    );
    expect(dialog.textContent).toContain(t("cloud:sync.cloud_vaults.delete_dialog.body"));
    expect(dialog.textContent).toContain(
      t("cloud:sync.cloud_vaults.delete_dialog.prompt", { text: "工作" }),
    );
    const submit = dialog.querySelector<HTMLButtonElement>("button[type=submit]")!;
    const input = dialog.querySelector<HTMLInputElement>("input")!;
    expect(submit.disabled).toBe(true);
    fireEvent.change(input, { target: { value: "工" } });
    expect(submit.disabled).toBe(true);
    fireEvent.change(input, { target: { value: "工作" } });
    expect(submit.disabled).toBe(false);
    fireEvent.click(submit);
    await waitFor(() =>
      expect(commandMock("syncCloudVaultDelete")).toHaveBeenCalledWith(AWAY.id),
    );
    await waitFor(() => expect(document.body.querySelector("[role=alertdialog]")).toBeNull());
    expect(commandMock("syncCloudVaults")).toHaveBeenCalledTimes(2);
  });

  it("名字读不出：照着输入 Rust 给的编号", async () => {
    const view = mount();
    await view.findByText(t("cloud:sync.cloud_vaults.name_unreadable"));
    fireEvent.click(
      rowOf(view, t("cloud:sync.cloud_vaults.name_unreadable")).querySelector<HTMLButtonElement>(
        ".luna-btn--danger",
      )!,
    );
    const dialog = deleteDialog();
    expect(dialog.textContent).toContain(
      t("cloud:sync.cloud_vaults.delete_dialog.prompt_unreadable", { text: "abcdef12" }),
    );
    fireEvent.change(dialog.querySelector("input")!, { target: { value: "abcdef12" } });
    expect(dialog.querySelector<HTMLButtonElement>("button[type=submit]")!.disabled).toBe(false);
  });

  it("组织成员删组织的库：说只有管理员能删，确认框留着", async () => {
    setCommandReply("syncCloudVaultDelete", { status: "error", error: "sync_manage_forbidden" });
    const view = mount();
    await view.findByText("工作");
    fireEvent.click(
      rowOf(view, "工作").querySelector<HTMLButtonElement>(".luna-btn--danger")!,
    );
    const dialog = deleteDialog();
    fireEvent.change(dialog.querySelector("input")!, { target: { value: "工作" } });
    fireEvent.click(dialog.querySelector<HTMLButtonElement>("button[type=submit]")!);
    await waitFor(() =>
      expect(deleteDialog().textContent).toContain(
        t("cloud:sync.error.sync_manage_forbidden"),
      ),
    );
  });

  it.each([
    ["sync_restore_over_capacity"],
    ["sync_restore_expired"],
  ])("恢复失败 %s 各有一句", async (code) => {
    setCommandReply("syncCloudVaultRestore", { status: "error", error: code });
    const view = mount();
    fireEvent.click(await view.findByText(t("cloud:sync.cloud_vaults.restore")));
    await waitFor(() =>
      expect(commandMock("syncCloudVaultRestore")).toHaveBeenCalledWith(DELETED.id),
    );
    expect(await view.findByText(t(`cloud:sync.error.${code}`))).toBeTruthy();
  });

  it("恢复成功：说可以重新下载或加入，并重取清单", async () => {
    const view = mount();
    fireEvent.click(await view.findByText(t("cloud:sync.cloud_vaults.restore")));
    expect(await view.findByText(t("cloud:sync.cloud_vaults.restored"))).toBeTruthy();
    expect(commandMock("syncCloudVaults")).toHaveBeenCalledTimes(2);
  });

  it("下载到本机：选择器标题说清会新建同名文件夹；选的存放位置原样交给 Rust；落点不行按稳定码说清", async () => {
    setCommandReply("pickFolder", { status: "ok", data: "/Users/me/Vault" });
    setCommandReply("syncCloudVaultDownload", { status: "error", error: "sync_folder_inside_vault" });
    const view = mount();
    await view.findByText("工作");
    fireEvent.click(
      rowOf(view, "工作").querySelector<HTMLButtonElement>(".luna-btn--secondary")!,
    );
    await waitFor(() =>
      expect(commandMock("syncCloudVaultDownload")).toHaveBeenCalledWith(AWAY.id, "/Users/me/Vault"),
    );
    expect(commandMock("pickFolder")).toHaveBeenCalledWith(
      t("cloud:sync.cloud_vaults.pick_parent", { name: "工作" }),
    );
    expect(await view.findByText(t("cloud:sync.error.sync_folder_inside_vault"))).toBeTruthy();

    // 成功时说的是 Rust 回来的那一层同名文件夹，不是选的存放位置。
    setCommandReply("pickFolder", { status: "ok", data: "/Users/me/Downloads" });
    setCommandReply("syncCloudVaultDownload", { status: "ok", data: "/Users/me/Downloads/工作" });
    fireEvent.click(
      rowOf(view, "工作").querySelector<HTMLButtonElement>(".luna-btn--secondary")!,
    );
    expect(
      await view.findByText(
        t("cloud:sync.cloud_vaults.downloaded", { path: "/Users/me/Downloads/工作" }),
      ),
    ).toBeTruthy();
  });

  it("取消选文件夹：什么都不发", async () => {
    setCommandReply("pickFolder", { status: "ok", data: null });
    const view = mount();
    await view.findByText("工作");
    fireEvent.click(
      rowOf(view, "工作").querySelector<HTMLButtonElement>(".luna-btn--secondary")!,
    );
    await waitFor(() => expect(commandMock("pickFolder")).toHaveBeenCalled());
    expect(commandMock("syncCloudVaultDownload")).not.toHaveBeenCalled();
  });

  it("清单取不到不伪装为空：给错误与重新读取", async () => {
    setCommandReply("syncCloudVaults", { status: "error", error: "network" });
    const view = mount();
    expect(await view.findByText(t("cloud:sync.error.network"))).toBeTruthy();
    expect(view.queryByText(t("cloud:sync.cloud_vaults.empty"))).toBeNull();
    setCommandReply("syncCloudVaults", { status: "ok", data: [] });
    fireEvent.click(view.getByText(t("cloud:sync.cloud_vaults.retry")));
    expect(await view.findByText(t("cloud:sync.cloud_vaults.empty"))).toBeTruthy();
  });
});
