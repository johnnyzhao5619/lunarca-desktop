// @vitest-environment happy-dom
/**
 * 账户数据权利（纵切 3 C6，PRD-13-020 / PRD-13-041）。
 *
 * 钉五件事：
 * 1. 导出按钮在途不可再点、状态只在点它的那一处说；取消对话框什么都不说；失败说原因；
 * 2. 删除确认先取后果预览：只列适用的几条（订阅 / 预付 / 保留年限 / 本机不受影响）；
 * 3. 预览取不到：说没取到、不编数字，但照常允许删除；
 * 4. 删除成功按回执的保留年限渲染（文案在前端，按语言）；`billing_unavailable` 说清什么都没删；
 * 5. 预览说「不能删」（组织唯一所有者，协议 v17）：说清下一步、确认不给点；预览之后才变成不能删的
 *    409 `last_org_owner` 说清什么都没删；
 * 6. 新文案中英齐备。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
vi.mock("../../ipc/cloud-bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type { AccountDeletionPreviewDto } from "../../ipc/cloud-bindings";
import { ConfirmHost } from "../../components/ConfirmHost";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import cloudEn from "../../i18n/locales/en-US/cloud.json";
import cloudZh from "../../i18n/locales/zh-CN/cloud.json";
import { AccountPanel } from "./AccountPanel";
import { AccountExport, DeletionConsequences, NO_EXPORT, type AccountExportControl } from "./AccountData";

afterEach(cleanup);

function control(overrides: Partial<AccountExportControl> = {}): AccountExportControl {
  return { ...NO_EXPORT, onExport: vi.fn(), ...overrides };
}

const ACTIVE: AccountDeletionPreviewDto = {
  deletion: { state: "allowed" },
  subscription: { state: "active", tier: "pro" },
  prepaid_credits: 250,
  billing_retention_years: 7,
};

const NOTHING: AccountDeletionPreviewDto = {
  deletion: { state: "allowed" },
  subscription: { state: "none" },
  prepaid_credits: 0,
  billing_retention_years: 7,
};

describe("导出按钮的几种状态", () => {
  it("空闲：说明导出里有什么、没有笔记；点下去交回是哪一处点的", () => {
    const exportControl = control();
    render(withI18n(<AccountExport control={exportControl} origin="account" />));
    expect(screen.getByText(t("cloud:export.desc"))).toBeTruthy();
    expect(t("cloud:export.desc")).toMatch(/笔记|notes/i);
    fireEvent.click(screen.getByRole("button", { name: t("cloud:export.button") }));
    expect(exportControl.onExport).toHaveBeenCalledWith("account");
  });

  it("在途：点它的那一处说「正在导出」，两处按钮都不可再点", () => {
    const pending = control({ pending: true, origin: "delete" });
    render(
      withI18n(
        <>
          <AccountExport control={pending} origin="account" />
          <AccountExport control={pending} origin="delete" />
        </>,
      ),
    );
    const here = screen.getByRole("button", { name: t("cloud:export.pending") });
    const there = screen.getByRole("button", { name: t("cloud:export.button") });
    expect((here as HTMLButtonElement).disabled).toBe(true);
    expect((there as HTMLButtonElement).disabled).toBe(true);
  });

  it("已保存：只在点它的那一处写出保存位置", () => {
    const saved = control({ origin: "delete", savedPath: "/Users/me/Desktop/export.json" });
    render(
      withI18n(
        <>
          <AccountExport control={saved} origin="account" />
          <AccountExport control={saved} origin="delete" />
        </>,
      ),
    );
    expect(screen.getAllByText(t("cloud:export.saved", { path: "/Users/me/Desktop/export.json" }))).toHaveLength(1);
  });

  it("失败：说原因（限流照既有那句说）", () => {
    render(
      withI18n(
        <AccountExport
          control={control({ origin: "account", error: { code: "rate_limited", retry_after_secs: 30 } })}
          origin="account"
        />,
      ),
    );
    expect(screen.getByText(t("cloud:error.rate_limited"))).toBeTruthy();
  });
});

describe("删除之后会怎样", () => {
  it("有订阅、有预付：四条都列，含 14 天退款提醒与保留年限", () => {
    render(withI18n(<DeletionConsequences preview={{ state: "loaded", preview: ACTIVE }} />));
    expect(screen.getByText(t("cloud:delete.consequence_subscription", { tier: "Pro" }))).toBeTruthy();
    expect(screen.getByText(t("cloud:delete.consequence_credits", { credits: "250" }))).toBeTruthy();
    expect(t("cloud:delete.consequence_credits", { credits: "250" })).toMatch(/14/);
    expect(screen.getByText(t("cloud:delete.consequence_retention", { count: 7 }))).toBeTruthy();
    expect(screen.getByText(t("cloud:delete.consequence_local"))).toBeTruthy();
  });

  it("没有订阅、没有预付：只列保留年限与本机不受影响", () => {
    const { container } = render(withI18n(<DeletionConsequences preview={{ state: "loaded", preview: NOTHING }} />));
    expect(container.querySelectorAll("li")).toHaveLength(2);
    expect(screen.queryByText(t("cloud:delete.consequence_subscription", { tier: "Pro" }))).toBeNull();
    expect(screen.getByText(t("cloud:delete.consequence_retention", { count: 7 }))).toBeTruthy();
  });

  it("档位认不出：走中性名，不把裸标识符写出来", () => {
    render(
      withI18n(
        <DeletionConsequences
          preview={{ state: "loaded", preview: { ...NOTHING, subscription: { state: "active", tier: "ultra" } } }}
        />,
      ),
    );
    expect(screen.getByText(t("cloud:delete.consequence_subscription", { tier: t("cloud:tier.other") }))).toBeTruthy();
  });

  it("取不到：说没取到与原因，不出现任何数字", () => {
    const { container } = render(
      withI18n(<DeletionConsequences preview={{ state: "failed", error: { code: "server", status: 500, error_code: "x" } }} />),
    );
    expect(container.textContent).toContain(t("cloud:delete.consequence_local"));
    expect(container.textContent).not.toMatch(/\d+ ?(年|years?|credits)/);
    expect(container.querySelector("[role=status]")?.textContent).toMatch(/仍然可以删除|still delete/);
  });

  it("不能删（组织唯一所有者）：先说下一步，后果照列", () => {
    render(
      withI18n(
        <DeletionConsequences
          preview={{
            state: "loaded",
            preview: { ...NOTHING, deletion: { state: "blocked", reason: "last_org_owner" } },
          }}
        />,
      ),
    );
    expect(screen.getByRole("status").textContent).toBe(t("cloud:delete.blocked.last_org_owner"));
    expect(screen.getByText(t("cloud:delete.consequence_retention", { count: 7 }))).toBeTruthy();
  });

  it("可以删：不出现「不能删」那一句", () => {
    render(withI18n(<DeletionConsequences preview={{ state: "loaded", preview: ACTIVE }} />));
    expect(screen.queryByText(t("cloud:delete.blocked.last_org_owner"))).toBeNull();
  });

  it("在取：说正在读取", () => {
    render(withI18n(<DeletionConsequences preview={{ state: "loading" }} />));
    expect(screen.getByText(t("cloud:delete.preview_loading"))).toBeTruthy();
  });
});

function signedIn() {
  setCommandReply("accountState", {
    status: "ok",
    data: {
      server: { kind: "official", origin: "https://cloud.example", display_name: null },
      phase: { phase: "signed_in", profile: { account_id: "a1", email: "a@b.c" } },
      incoming_link: null,
    },
  });
  setCommandReply("devicesList", { status: "ok", data: [] });
  setCommandReply("entitlements", { status: "error", error: { code: "server", status: 500, error_code: "x" } });
  setCommandReply("usage", { status: "ok", data: { ledgers: [], spend_cap: null } });
  setCommandReply("billingOffer", { status: "ok", data: { state: "disabled" } });
}

function renderPanel() {
  return render(withI18n(<ConfirmHost><AccountPanel /></ConfirmHost>));
}

async function openDeletion() {
  fireEvent.click(await screen.findByRole("button", { name: t("cloud:delete.button") }));
}

function confirmButton() {
  return screen.getByRole("button", { name: t("cloud:delete.confirm") }) as HTMLButtonElement;
}

describe("账户页接线", () => {
  beforeEach(() => resetBindingsDouble());

  it("导出：交给 Rust；写好之后说保存在哪", async () => {
    signedIn();
    setCommandReply("accountExport", { status: "ok", data: { outcome: "saved", path: "/tmp/lunarca.json" } });
    renderPanel();
    fireEvent.click(await screen.findByRole("button", { name: t("cloud:export.button") }));
    await screen.findByText(t("cloud:export.saved", { path: "/tmp/lunarca.json" }));
    expect(commandMock("accountExport")).toHaveBeenCalledTimes(1);
  });

  it("导出：取消对话框什么都不说", async () => {
    signedIn();
    setCommandReply("accountExport", { status: "ok", data: { outcome: "cancelled" } });
    renderPanel();
    const button = await screen.findByRole("button", { name: t("cloud:export.button") });
    fireEvent.click(button);
    await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
    expect(document.body.textContent).not.toContain(t("cloud:export.saved", { path: "" }).trim());
  });

  it("删除：先取预览、摆出后果，确认后按回执的保留年限说", async () => {
    signedIn();
    setCommandReply("accountDeletionPreview", { status: "ok", data: ACTIVE });
    setCommandReply("accountDelete", { status: "ok", data: { billing_retention_years: 7 } });
    renderPanel();
    await openDeletion();
    await screen.findByText(t("cloud:delete.consequence_subscription", { tier: "Pro" }));
    expect(commandMock("accountDeletionPreview")).toHaveBeenCalledTimes(1);
    expect(commandMock("accountDelete")).not.toHaveBeenCalled();
    // 删除之前给「先导出」。
    expect(screen.getByText(t("cloud:delete.export_first"))).toBeTruthy();
    fireEvent.click(confirmButton());
    await screen.findByText(t("cloud:delete.done", { count: 7 }));
    expect(commandMock("accountDelete")).toHaveBeenCalledTimes(1);
  });

  it("预览取不到：说没取到，但照常可以删除", async () => {
    signedIn();
    setCommandReply("accountDeletionPreview", { status: "error", error: { code: "rate_limited", retry_after_secs: null } });
    setCommandReply("accountDelete", { status: "ok", data: { billing_retention_years: 10 } });
    renderPanel();
    await openDeletion();
    await screen.findByText(
      t("cloud:delete.preview_failed", { reason: t("cloud:error.rate_limited") }),
    );
    expect(confirmButton().disabled).toBe(false);
    fireEvent.click(confirmButton());
    await screen.findByText(t("cloud:delete.done", { count: 10 }));
  });

  it("预览在途：确认按钮先不给点", async () => {
    signedIn();
    setCommandReply("accountDeletionPreview", () => new Promise(() => undefined));
    renderPanel();
    await openDeletion();
    await screen.findByText(t("cloud:delete.preview_loading"));
    expect(confirmButton().disabled).toBe(true);
  });

  it("组织唯一所有者：预览说不能删，说清下一步，确认不给点、一次删除请求都不发", async () => {
    signedIn();
    setCommandReply("accountDeletionPreview", {
      status: "ok",
      data: { ...ACTIVE, deletion: { state: "blocked", reason: "last_org_owner" } },
    });
    renderPanel();
    await openDeletion();
    await screen.findByText(
      "你是组织里唯一的所有者，组织还有其他成员。请先把所有者转给其他成员，或请管理员处理后再删除账户。",
    );
    expect(confirmButton().disabled).toBe(true);
    fireEvent.click(confirmButton());
    expect(commandMock("accountDelete")).not.toHaveBeenCalled();
    // 取消照常可点：不能删不等于把人困在确认区里。
    expect(
      (screen.getByRole("button", { name: t("cloud:delete.cancel") }) as HTMLButtonElement).disabled,
    ).toBe(false);
  });

  it("预览之后才成了组织唯一所有者（409）：说清什么都没删，账户原样在", async () => {
    signedIn();
    setCommandReply("accountDeletionPreview", { status: "ok", data: NOTHING });
    setCommandReply("accountDelete", { status: "error", error: { code: "last_org_owner" } });
    renderPanel();
    await openDeletion();
    await screen.findByText(t("cloud:delete.consequence_local"));
    expect(confirmButton().disabled).toBe(false);
    fireEvent.click(confirmButton());
    await screen.findByText(t("cloud:error.last_org_owner"));
    expect(screen.queryByText(t("cloud:delete.done", { count: 7 }))).toBeNull();
    expect(screen.getByRole("button", { name: t("cloud:export.button") })).toBeTruthy();
  });

  it("订阅退不掉：说清什么都没删，账户原样在", async () => {
    signedIn();
    setCommandReply("accountDeletionPreview", { status: "ok", data: NOTHING });
    setCommandReply("accountDelete", { status: "error", error: { code: "billing_unavailable" } });
    renderPanel();
    await openDeletion();
    await screen.findByText(t("cloud:delete.consequence_local"));
    fireEvent.click(confirmButton());
    await screen.findByText(t("cloud:error.billing_unavailable"));
    expect(screen.queryByText(t("cloud:delete.done", { count: 7 }))).toBeNull();
    // 还是登录态：导出按钮仍在。
    expect(screen.getByRole("button", { name: t("cloud:export.button") })).toBeTruthy();
  });
});

describe("文案", () => {
  it("新文案中英齐备", () => {
    for (const bundle of [cloudZh, cloudEn]) {
      for (const key of ["desc", "button", "pending", "saved"] as const) {
        expect(bundle.export[key], `缺 export.${key}`).toBeTypeOf("string");
      }
      for (const key of [
        "preview_loading",
        "preview_failed",
        "consequence_subscription",
        "consequence_credits",
        "consequence_retention",
        "consequence_local",
        "export_first",
        "done",
      ] as const) {
        expect(bundle.delete[key], `缺 delete.${key}`).toBeTypeOf("string");
      }
      expect(bundle.delete.blocked.last_org_owner, "缺 delete.blocked.last_org_owner").toBeTypeOf("string");
    }
    // 英文按年数分单复数。
    expect(cloudEn.delete.done_one).toContain("{{count}} year ");
    expect(cloudEn.delete.consequence_retention_one).toContain("{{count}} year ");
  });

  it("回执里不再有服务端写死的一句话", () => {
    for (const bundle of [cloudZh, cloudEn]) {
      expect(bundle.delete.done).not.toContain("{{retained}}");
    }
  });
});
