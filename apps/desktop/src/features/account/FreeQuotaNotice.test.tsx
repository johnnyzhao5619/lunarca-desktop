// @vitest-environment happy-dom
/**
 * 免费额度说明（纵切 3 C5，PRD-13-018 L1 / L2）。
 *
 * 钉四件事：
 * 1. 只在被扣下时出现；`not_applicable` / `granted` 一个字都不多说；
 * 2. 同设备（L1）：说清每台设备一个账户、预付与订阅照常可用、可以联系支持申诉——**没有表单、没有输入框**
 *    （申诉不得要求提供更多个人信息），也没有验证按钮；
 * 3. 要验证（L2）：说清不扣费，按钮交给 Rust 打开验证页，回到窗口整页重取；
 * 4. 已不需要验证 → 重取；本部署不卖 → 如实说。锚点在，失败卡的深链才落得到。
 */

import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
vi.mock("../../ipc/cloud-bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type { EntitlementsDto, FreeQuotaDto } from "../../ipc/cloud-bindings";
import { ConfirmHost } from "../../components/ConfirmHost";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { AccountPanel } from "./AccountPanel";
import { FREE_QUOTA_SECTION, FreeQuotaNotice, type FreeQuotaVerify } from "./FreeQuotaNotice";

const SHARED: FreeQuotaDto = { state: "withheld", reason: "shared_device" };
const VERIFY: FreeQuotaDto = { state: "withheld", reason: "verification_required" };

function idle(overrides: Partial<FreeQuotaVerify> = {}): FreeQuotaVerify {
  return { pending: false, error: null, opened: false, onVerify: vi.fn(), ...overrides };
}

afterEach(cleanup);

describe("免费额度说明的三种处境", () => {
  it("不适用 / 照常发放：这一块不出现", () => {
    for (const freeQuota of [{ state: "not_applicable" }, { state: "granted" }] as const) {
      const { container } = render(withI18n(<FreeQuotaNotice freeQuota={freeQuota} verify={idle()} />));
      expect(container.textContent).toBe("");
      cleanup();
    }
    const { container } = render(withI18n(<FreeQuotaNotice freeQuota={null} verify={idle()} />));
    expect(container.textContent).toBe("");
  });

  it("同一台设备已有账户领过：说清规则与申诉路径；没有按钮、没有任何输入框", () => {
    render(withI18n(<FreeQuotaNotice freeQuota={SHARED} verify={idle()} />));
    expect(screen.getByText(t("cloud:usage.free_quota.shared_device"))).toBeTruthy();
    expect(screen.getByText(t("cloud:usage.free_quota.shared_device_appeal"))).toBeTruthy();
    expect(screen.queryByRole("button")).toBeNull();
    expect(document.querySelector("input, textarea, form")).toBeNull();
    expect(document.getElementById(`settings-section-${FREE_QUOTA_SECTION}`)).not.toBeNull();
  });

  it("要验证付款方式：说清不扣费，按钮交回动作；在途时不可再点", () => {
    const verify = idle();
    const { rerender } = render(withI18n(<FreeQuotaNotice freeQuota={VERIFY} verify={verify} />));
    expect(screen.getByText(t("cloud:usage.free_quota.verification_required"))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("cloud:usage.free_quota.verify") }));
    expect(verify.onVerify).toHaveBeenCalledTimes(1);
    rerender(withI18n(<FreeQuotaNotice freeQuota={VERIFY} verify={idle({ pending: true })} />));
    expect(
      (screen.getByRole("button", { name: t("cloud:usage.free_quota.verify") }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it("按钮文案说清不扣费（两种语言都写在按钮上，而不是只写在说明里）", () => {
    expect(t("cloud:usage.free_quota.verify")).toMatch(/不扣费|no charge/i);
  });
});

function entitlements(freeQuota: FreeQuotaDto): EntitlementsDto {
  return {
    tier: "free",
    entitlements: [],
    lease_verified: true,
    lease_expires_at: null,
    lease_error: null,
    capabilities: null,
    billing_standing: { state: "good" },
    free_quota: freeQuota,
  };
}

function signedIn(freeQuota: FreeQuotaDto) {
  setCommandReply("accountState", {
    status: "ok",
    data: {
      server: { kind: "official", origin: "https://cloud.example", display_name: null },
      phase: { phase: "signed_in", profile: { account_id: "a1", email: "a@b.c" } },
      incoming_link: null,
    },
  });
  setCommandReply("devicesList", { status: "ok", data: [] });
  setCommandReply("entitlements", { status: "ok", data: entitlements(freeQuota) });
  setCommandReply("usage", { status: "ok", data: { ledgers: [], spend_cap: null } });
  setCommandReply("billingOffer", { status: "ok", data: { state: "disabled" } });
}

function renderPanel() {
  return render(withI18n(<ConfirmHost><AccountPanel /></ConfirmHost>));
}

describe("免费额度 · 账户页接线", () => {
  beforeEach(() => resetBindingsDouble());

  it("验证：交给 Rust 打开浏览器；回到应用即重取整页", async () => {
    signedIn(VERIFY);
    setCommandReply("billingVerifyCard", { status: "ok", data: null });
    renderPanel();
    fireEvent.click(await screen.findByRole("button", { name: t("cloud:usage.free_quota.verify") }));
    await screen.findByText(t("cloud:usage.free_quota.verify_opened"));
    expect(commandMock("billingVerifyCard")).toHaveBeenCalledTimes(1);
    const before = commandMock("entitlements").mock.calls.length;
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    await waitFor(() => expect(commandMock("entitlements").mock.calls.length).toBe(before + 1));
    await waitFor(() => expect(screen.queryByText(t("cloud:usage.free_quota.verify_opened"))).toBeNull());
  });

  it("已不需要验证：说一句并重取（说明随权益消失）", async () => {
    signedIn(VERIFY);
    setCommandReply("billingVerifyCard", { status: "error", error: { code: "verification_not_required" } });
    renderPanel();
    fireEvent.click(await screen.findByRole("button", { name: t("cloud:usage.free_quota.verify") }));
    await screen.findByText(t("cloud:error.verification_not_required"));
    await waitFor(() => expect(commandMock("entitlements").mock.calls.length).toBeGreaterThan(1));
  });

  it("本部署不卖：如实说，不重取", async () => {
    signedIn(VERIFY);
    setCommandReply("billingVerifyCard", { status: "error", error: { code: "billing_disabled" } });
    renderPanel();
    fireEvent.click(await screen.findByRole("button", { name: t("cloud:usage.free_quota.verify") }));
    await screen.findByText(t("cloud:error.billing_disabled"));
    expect(commandMock("entitlements")).toHaveBeenCalledTimes(1);
  });

  it("同设备：账户页上是规则与申诉，没有验证按钮", async () => {
    signedIn(SHARED);
    renderPanel();
    await screen.findByText(t("cloud:usage.free_quota.shared_device"));
    expect(screen.queryByRole("button", { name: t("cloud:usage.free_quota.verify") })).toBeNull();
  });
});
