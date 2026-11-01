// @vitest-environment happy-dom
/**
 * 账户页「升级与购买」（纵切 3 C2）。
 *
 * 钉四件事：
 * 1. **卖不卖由服务端说了算**：未启用收单（私有化）时整节不渲染，一个死按钮都不留；
 * 2. 已订阅不再给「订阅」（再结一次账会得到第二份订阅），改档只走「管理订阅」；档位取不到同样不给；
 * 3. 价目按服务端给的币种与最小单位格式化，账单口径逐条来自服务端规则；
 * 4. 整页接线：管理订阅 404（从没付过款）如实说出来；结账打开浏览器后，回到应用即重取。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
vi.mock("../../ipc/cloud-bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type { BillingOfferDto, BillingPlansDto, EntitlementsDto } from "../../ipc/cloud-bindings";
import { ConfirmHost } from "../../components/ConfirmHost";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { formatDate } from "../../lib/format-datetime";
import { formatMinorUnits } from "../../lib/format-money";
import { AccountPanel } from "./AccountPanel";
import { BillingSection } from "./BillingSection";

const PLANS: BillingPlansDto = {
  subscriptions: [
    {
      id: "plan-pro",
      tier: "pro",
      display_name: "Pro",
      price: { unit_amount: 999, currency: "usd" },
      interval: "month",
      current: false,
      included: {
        period: "monthly",
        dimensions: [
          { dimension: "tokens_out", limit: 2_000_000 },
          { dimension: "egress_bytes", limit: null },
        ],
      },
    },
    {
      id: "plan-max",
      tier: "max",
      display_name: "Max",
      price: { unit_amount: 2999, currency: "usd" },
      interval: "month",
      current: false,
      included: { period: "monthly", dimensions: [] },
    },
  ],
  prepaid_packs: [
    { id: "pack-600", display_name: "600 credits", price: { unit_amount: 500, currency: "usd" }, credits: 600 },
  ],
  rules: {
    ledger_order: ["subscription", "prepaid"],
    splits_across_ledgers: false,
    prepaid_credits_expire: false,
    minimum_billable_units: [{ dimension: "asr_seconds", unit: 60 }],
  },
};

const ENABLED: BillingOfferDto = {
  state: "enabled",
  plans: PLANS,
  standing: "not_subscribed",
  payment: { state: "good" },
  purchasable: true,
};

/** 已在 Pro 上（Rust 标好的 `current` 与处境）。 */
const SUBSCRIBED: BillingOfferDto = {
  state: "enabled",
  standing: "subscribed",
  payment: { state: "good" },
  purchasable: true,
  plans: {
    ...PLANS,
    subscriptions: PLANS.subscriptions.map((plan) => ({ ...plan, current: plan.tier === "pro" })),
  },
};
const noop = () => {};

function section(props: Partial<Parameters<typeof BillingSection>[0]> = {}) {
  return render(
    withI18n(<BillingSection offer={ENABLED} onCheckout={noop} onPortal={noop} {...props} />),
  );
}

afterEach(() => {
  cleanup();
  resetBindingsDouble();
});

describe("升级与购买 · 呈现", () => {
  it("未启用收单（私有化）：整节不渲染，没有任何升级 / 购买 / 管理入口", () => {
    const { container } = section({ offer: { state: "disabled" } });
    expect(container.innerHTML).toBe("");
    expect(screen.queryByText(t("cloud:billing.title"))).toBeNull();
  });

  it("还没取回时不渲染（私有化部署不闪一下空节）", () => {
    const { container } = section({ offer: null });
    expect(container.innerHTML).toBe("");
  });

  it("未订阅：每个订阅档可订阅、每个流量包可购买，价目按币种与最小单位格式化", () => {
    const onCheckout = vi.fn();
    section({ onCheckout });
    expect(screen.getByRole("heading", { name: t("cloud:billing.title") })).toBeTruthy();
    expect(screen.getAllByRole("button", { name: t("cloud:billing.subscribe") })).toHaveLength(2);
    expect(screen.queryByRole("button", { name: t("cloud:billing.manage") })).toBeNull();

    const price = t("cloud:billing.price_per.month", { price: formatMinorUnits(999, "usd", "zh-CN") });
    expect(document.body.textContent).toContain(price);
    expect(document.body.textContent).toContain("US$9.99");
    expect(document.body.textContent).toContain(
      t("cloud:billing.pack_detail", { price: formatMinorUnits(500, "usd", "zh-CN"), credits: "600" }),
    );
    // 「不限」写成「不限」而不是 0 或破折号。
    expect(document.body.textContent).toContain(
      t("cloud:billing.included_item", {
        dimension: t("cloud:usage.dimension.egress_bytes"),
        amount: t("cloud:billing.unlimited"),
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: t("cloud:billing.buy") }));
    expect(onCheckout).toHaveBeenCalledWith("pack-600");
  });

  it("已订阅：当前档有徽标，不再给「订阅」（会得到第二份订阅），改档走「管理订阅」", () => {
    const onPortal = vi.fn();
    section({ offer: SUBSCRIBED, onPortal });
    expect(screen.getByText(t("cloud:billing.current"))).toBeTruthy();
    expect(screen.queryByRole("button", { name: t("cloud:billing.subscribe") })).toBeNull();
    expect(screen.getByText(t("cloud:billing.switch_hint"))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("cloud:billing.manage") }));
    expect(onPortal).toHaveBeenCalledTimes(1);
    // 流量包任一档都能叠加购买。
    expect(screen.getByRole("button", { name: t("cloud:billing.buy") })).toBeTruthy();
  });

  it("付款正常或不知道：不出付款处境横幅", () => {
    section();
    section({ offer: { ...ENABLED, payment: null } });
    expect(screen.queryByText(t("cloud:billing.standing.frozen"))).toBeNull();
    expect(
      screen.queryByRole("button", { name: t("cloud:billing.standing.past_due_action") }),
    ).toBeNull();
  });

  it("催缴（纵切 3 C3）：横幅写明按界面语言格式化的截止日期，「管理订阅」打开账单页；照常可买", () => {
    const onPortal = vi.fn();
    const graceEndsAt = "2026-10-28T12:00:00Z";
    section({
      offer: { ...SUBSCRIBED, payment: { state: "past_due", grace_ends_at: graceEndsAt } },
      onPortal,
    });
    const date = formatDate(Date.parse(graceEndsAt), "zh-CN");
    expect(screen.getByText(t("cloud:billing.standing.past_due", { date }))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("cloud:billing.standing.past_due_action") }));
    expect(onPortal).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: t("cloud:billing.buy") })).toBeTruthy();
  });

  it("冻结（纵切 3 C3）：只说明、联系支持，不给任何下单入口（Rust 标的 purchasable 为假）", () => {
    section({ offer: { ...ENABLED, payment: { state: "frozen" }, purchasable: false } });
    expect(screen.getByText(t("cloud:billing.standing.frozen"))).toBeTruthy();
    expect(screen.queryByRole("button", { name: t("cloud:billing.subscribe") })).toBeNull();
    expect(screen.queryByRole("button", { name: t("cloud:billing.buy") })).toBeNull();
    // 价目与账单口径照常可读。
    expect(screen.getByText("Pro")).toBeTruthy();
    expect(screen.getByText(t("cloud:billing.rules_title"))).toBeTruthy();
  });

  it("档位取不到：订阅与管理都不给，说明原因", () => {
    section({ offer: { ...ENABLED, standing: "unknown" } });
    expect(screen.queryByRole("button", { name: t("cloud:billing.subscribe") })).toBeNull();
    expect(screen.queryByRole("button", { name: t("cloud:billing.manage") })).toBeNull();
    expect(screen.getByText(t("cloud:billing.tier_unknown_hint"))).toBeTruthy();
  });

  it("账单口径逐条来自服务端规则：扣减顺序、不拆分、不过期、最小计费单元", () => {
    section();
    const text = document.body.textContent ?? "";
    const order = [t("cloud:usage.ledger_subscription"), t("cloud:usage.ledger_prepaid")].join(
      t("cloud:billing.rules.order_separator"),
    );
    expect(text).toContain(t("cloud:billing.rules.order", { order }));
    expect(text).toContain(t("cloud:billing.rules.no_split"));
    expect(text).toContain(t("cloud:billing.rules.prepaid_never_expires"));
    expect(text).toContain(
      t("cloud:billing.rules.minimum_unit", { dimension: t("cloud:usage.dimension.asr_seconds"), unit: "60" }),
    );
  });

  it("结账进行中按钮全部禁用，免得连点出两张付款页", () => {
    section({ pending: { kind: "checkout", planId: "pack-600" } });
    for (const button of screen.getAllByRole("button")) {
      expect((button as HTMLButtonElement).disabled).toBe(true);
    }
  });
});

// ---- 整页接线 ----

function entitlements(tier: string): EntitlementsDto {
  return {
    tier,
    entitlements: [],
    lease_verified: true,
    lease_expires_at: null,
    lease_error: null,
    capabilities: null,
    billing_standing: { state: "good" },
    free_quota: { state: "not_applicable" },
  };
}

function signedIn(tier: string, offer: BillingOfferDto) {
  setCommandReply("accountState", {
    status: "ok",
    data: {
      server: { kind: "official", origin: "https://cloud.example", display_name: null },
      phase: { phase: "signed_in", profile: { account_id: "a1", email: "a@b.c" } },
      incoming_link: null,
    },
  });
  setCommandReply("devicesList", { status: "ok", data: [] });
  setCommandReply("entitlements", { status: "ok", data: entitlements(tier) });
  setCommandReply("usage", { status: "ok", data: { ledgers: [], spend_cap: null } });
  setCommandReply("billingOffer", { status: "ok", data: offer });
}

function renderPanel() {
  return render(withI18n(<ConfirmHost><AccountPanel /></ConfirmHost>));
}

describe("升级与购买 · 账户页接线", () => {
  beforeEach(() => resetBindingsDouble());

  it("私有化（未启用收单）：账户页没有这一节", async () => {
    signedIn("organization", { state: "disabled" });
    renderPanel();
    await waitFor(() => expect(commandMock("billingOffer")).toHaveBeenCalled());
    await screen.findByText(t("cloud:usage.empty"));
    expect(screen.queryByText(t("cloud:billing.title"))).toBeNull();
    expect(screen.queryByRole("button", { name: t("cloud:billing.buy") })).toBeNull();
  });

  it("管理订阅 404（从没付过款）：如实说没有可管理的订阅，而不是「这里没有 Lunarca 服务端」", async () => {
    signedIn("pro", SUBSCRIBED);
    setCommandReply("billingPortal", { status: "error", error: { code: "no_billing_account" } });
    renderPanel();
    fireEvent.click(await screen.findByRole("button", { name: t("cloud:billing.manage") }));
    await screen.findByText(t("cloud:error.no_billing_account"));
    expect(screen.queryByText(t("cloud:error.server_action.not_found"), { exact: false })).toBeNull();
  });

  it("结账交给 Rust 打开浏览器；回到应用（窗口重新获得焦点）即重取整页", async () => {
    signedIn("free", ENABLED);
    setCommandReply("billingCheckout", { status: "ok", data: null });
    renderPanel();
    fireEvent.click(await screen.findByRole("button", { name: t("cloud:billing.buy") }));
    await screen.findByText(t("cloud:billing.opened"));
    expect(commandMock("billingCheckout")).toHaveBeenCalledWith("pack-600");
    const before = commandMock("billingOffer").mock.calls.length;
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    await waitFor(() => expect(commandMock("billingOffer").mock.calls.length).toBe(before + 1));
    expect(commandMock("entitlements").mock.calls.length).toBeGreaterThan(1);
    await waitFor(() => expect(screen.queryByText(t("cloud:billing.opened"))).toBeNull());
  });

  it("那一项已下架：说清楚并重取价目", async () => {
    signedIn("free", ENABLED);
    setCommandReply("billingCheckout", { status: "error", error: { code: "plan_unavailable" } });
    renderPanel();
    fireEvent.click(await screen.findByRole("button", { name: t("cloud:billing.buy") }));
    await screen.findByText(t("cloud:error.plan_unavailable"));
    await waitFor(() => expect(commandMock("billingOffer").mock.calls.length).toBeGreaterThan(1));
  });
});
