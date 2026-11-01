// @vitest-environment happy-dom
/**
 * 本月预付上限（纵切 3 C4，PRD-13-040 ④）。
 *
 * 钉四件事：
 * 1. 设了 / 没设两种状态各说各的：设了给进度、「已花 / 上限」与服务端判定的徽标；没设说「未设上限」；
 * 2. 输入框空着或不是数时不给保存——**绝不能把「没填」当成「清除」交出去**；清除是单独一颗按钮；
 * 3. 交回去的是 credits（换算与校验在 Rust），设完整页重取；失败如实说原因；
 * 4. 没有预付也没设过上限（Rust 给 `null`）时这一块不出现；锚点在，深链才落得到。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
vi.mock("../../ipc/cloud-bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type { SpendCapDto, UsageDto } from "../../ipc/cloud-bindings";
import { ConfirmHost } from "../../components/ConfirmHost";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { AccountPanel } from "./AccountPanel";
import { SPEND_CAP_SECTION, SpendCapControl } from "./SpendCapControl";

const ENDS = "2026-11-01T00:00:00Z";
const SET: SpendCapDto = {
  state: "set",
  limit_credits: 50,
  spent_credits: 49,
  period_ends_at: ENDS,
  alert: "p95",
};
const UNSET: SpendCapDto = { state: "unset", spent_credits: 1.5, period_ends_at: ENDS };

afterEach(cleanup);

describe("上限控件的两种状态", () => {
  it("设了上限：已花 / 上限、进度与服务端给的徽标；有「清除上限」", () => {
    render(withI18n(<SpendCapControl cap={SET} onSave={vi.fn()} />));
    expect(screen.getByText(t("cloud:usage.spend_cap.amount", { spent: "49", limit: "50" }))).toBeTruthy();
    expect(screen.getByText(t("cloud:usage.alert.p95"))).toBeTruthy();
    expect(document.querySelector("progress")).not.toBeNull();
    expect(screen.getByRole("button", { name: t("cloud:usage.spend_cap.clear") })).toBeTruthy();
    expect((screen.getByLabelText(t("cloud:usage.spend_cap.input_label")) as HTMLInputElement).value).toBe("50");
    // 上限只管预付：说明里讲清订阅额度不受影响。
    expect(screen.getByText(t("cloud:usage.spend_cap.hint"))).toBeTruthy();
  });

  it("没设上限：照样报本月花了多少并说「未设上限」，没有进度条、没有清除", () => {
    render(withI18n(<SpendCapControl cap={UNSET} onSave={vi.fn()} />));
    expect(screen.getByText(t("cloud:usage.spend_cap.amount_unset", { spent: "1.5" }))).toBeTruthy();
    expect(document.querySelector("progress")).toBeNull();
    expect(screen.queryByRole("button", { name: t("cloud:usage.spend_cap.clear") })).toBeNull();
  });

  it("用尽有自己的一档徽标，不说成「即将耗尽」", () => {
    render(withI18n(<SpendCapControl cap={{ ...SET, state: "set", alert: "exhausted" }} onSave={vi.fn()} />));
    expect(screen.getByText(t("cloud:usage.alert.exhausted"))).toBeTruthy();
    expect(screen.queryByText(t("cloud:usage.alert.p95"))).toBeNull();
  });

  it("锚点就是深链的尾段（失败卡、不足提示与上限通知都落在这里）", () => {
    render(withI18n(<SpendCapControl cap={UNSET} onSave={vi.fn()} />));
    expect(document.getElementById(`settings-section-${SPEND_CAP_SECTION}`)).not.toBeNull();
  });
});

describe("上限控件交回去的数", () => {
  it("空着不给保存——「没填」绝不能被当成「清除」", () => {
    const onSave = vi.fn();
    render(withI18n(<SpendCapControl cap={UNSET} onSave={onSave} />));
    const save = screen.getByRole("button", { name: t("cloud:usage.spend_cap.save") }) as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    fireEvent.change(screen.getByLabelText(t("cloud:usage.spend_cap.input_label")), { target: { value: "25" } });
    expect(save.disabled).toBe(false);
    fireEvent.click(save);
    expect(onSave).toHaveBeenCalledWith(25);
  });

  it("没改动不给保存；清除交回 null", () => {
    const onSave = vi.fn();
    render(withI18n(<SpendCapControl cap={SET} onSave={onSave} />));
    const save = screen.getByRole("button", { name: t("cloud:usage.spend_cap.save") }) as HTMLButtonElement;
    expect(save.disabled, "与当前上限相同，保存只会多一次请求").toBe(true);
    fireEvent.click(screen.getByRole("button", { name: t("cloud:usage.spend_cap.clear") }));
    expect(onSave).toHaveBeenCalledWith(null);
  });

  it("在途时两颗按钮与输入框都不可用", () => {
    render(withI18n(<SpendCapControl cap={SET} pending onSave={vi.fn()} />));
    for (const name of [t("cloud:usage.spend_cap.save"), t("cloud:usage.spend_cap.clear")]) {
      expect((screen.getByRole("button", { name }) as HTMLButtonElement).disabled).toBe(true);
    }
    expect((screen.getByLabelText(t("cloud:usage.spend_cap.input_label")) as HTMLInputElement).disabled).toBe(true);
  });
});

function signedIn(usage: UsageDto) {
  setCommandReply("accountState", {
    status: "ok",
    data: {
      server: { kind: "official", origin: "https://cloud.example", display_name: null },
      phase: { phase: "signed_in", profile: { account_id: "a1", email: "a@b.c" } },
      incoming_link: null,
    },
  });
  setCommandReply("devicesList", { status: "ok", data: [] });
  setCommandReply("entitlements", { status: "error", error: { code: "not_logged_in" } });
  setCommandReply("usage", { status: "ok", data: usage });
  setCommandReply("billingOffer", { status: "ok", data: { state: "disabled" } });
}

function renderPanel() {
  return render(withI18n(<ConfirmHost><AccountPanel /></ConfirmHost>));
}

describe("上限 · 账户页接线", () => {
  beforeEach(() => resetBindingsDouble());

  it("Rust 说没有上限可谈（无预付、未设）：这一块不出现；用量段锚点在", async () => {
    signedIn({ ledgers: [], spend_cap: null });
    renderPanel();
    await screen.findByText(t("cloud:usage.empty"));
    expect(screen.queryByText(t("cloud:usage.spend_cap.input_label"))).toBeNull();
    expect(document.getElementById("settings-section-usage")).not.toBeNull();
  });

  it("设上限：把 credits 交给 Rust，设完重取整页", async () => {
    signedIn({ ledgers: [], spend_cap: UNSET });
    setCommandReply("spendCapSet", { status: "ok", data: { ...SET, alert: "none" } });
    renderPanel();
    const input = await screen.findByLabelText(t("cloud:usage.spend_cap.input_label"));
    fireEvent.change(input, { target: { value: "50" } });
    fireEvent.click(screen.getByRole("button", { name: t("cloud:usage.spend_cap.save") }));
    await waitFor(() => expect(commandMock("spendCapSet")).toHaveBeenCalledWith(50));
    await waitFor(() => expect(commandMock("usage")).toHaveBeenCalledTimes(2));
  });

  it("设失败：如实说原因，不重取、不假装设上了", async () => {
    signedIn({ ledgers: [], spend_cap: UNSET });
    setCommandReply("spendCapSet", { status: "error", error: { code: "invalid_spend_cap" } });
    renderPanel();
    const input = await screen.findByLabelText(t("cloud:usage.spend_cap.input_label"));
    fireEvent.change(input, { target: { value: "-1" } });
    fireEvent.click(screen.getByRole("button", { name: t("cloud:usage.spend_cap.save") }));
    await screen.findByText(t("cloud:error.invalid_spend_cap"));
    expect(commandMock("usage")).toHaveBeenCalledTimes(1);
  });
});
