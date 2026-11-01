// @vitest-environment happy-dom
/**
 * 「额度不足」的修复按钮（PRD-13-019 升级引导，纵切 3 C2）。
 *
 * 失败卡与 HUD 回执共用 `useFailureGuidance` 一处：卖额度的服务器上给「升级或购买额度」并落在
 * 账户页「升级与购买」一节；私有化部署（不卖）、问不到时照旧「打开账户设置」——不给一扇画在
 * 墙上的门。与收单无关的码一个字节都不多问。
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../ipc/bindings", () => import("../ipc/bindings-test-double"));
vi.mock("../ipc/cloud-bindings", () => import("../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../ipc/bindings-test-double";
import { FailureCard } from "../components/ui/FailureCard";
import { HudReceipt } from "../components/ui/HudReceipt";
import { t, withI18n } from "../i18n/i18n-test-fixture";
import { failureMessage } from "../i18n/failure";
import { NAVIGATE_EVENT } from "../lib/navigate";

afterEach(() => {
  cleanup();
  resetBindingsDouble();
});

/** 截住 `goTo`：主窗的导航处理器收下即 `preventDefault`。 */
function captureNavigation(): string[] {
  const routes: string[] = [];
  window.addEventListener(NAVIGATE_EVENT, (event) => {
    routes.push((event as CustomEvent<string>).detail);
    event.preventDefault();
  });
  return routes;
}

describe("额度不足的修复动作跟着服务器卖不卖走", () => {
  it("卖额度：失败卡给「升级或购买额度」，落在账户页「升级与购买」一节", async () => {
    setCommandReply("billingAvailable", { status: "ok", data: true });
    const routes = captureNavigation();
    render(withI18n(<FailureCard code="QUOTA_EXHAUSTED" />));
    const button = await screen.findByRole("button", { name: t("common:failure.action.upgrade") });
    fireEvent.click(button);
    expect(routes).toContain("settings://account/billing");
  });

  it("不卖（私有化）：照旧「打开账户设置」，不出现升级", async () => {
    setCommandReply("billingAvailable", { status: "ok", data: false });
    render(withI18n(<FailureCard code="QUOTA_EXHAUSTED" />));
    await waitFor(() => expect(commandMock("billingAvailable")).toHaveBeenCalled());
    expect(screen.getByRole("button", { name: t("common:failure.action.open_account") })).toBeTruthy();
    expect(screen.queryByRole("button", { name: t("common:failure.action.upgrade") })).toBeNull();
  });

  it("问不到（未登录 / 严格本地 / 连不上）按不卖处理", async () => {
    setCommandReply("billingAvailable", { status: "error", error: { code: "strict_local_blocked" } });
    render(withI18n(<FailureCard code="QUOTA_EXHAUSTED" />));
    await waitFor(() => expect(commandMock("billingAvailable")).toHaveBeenCalled());
    expect(screen.getByRole("button", { name: t("common:failure.action.open_account") })).toBeTruthy();
  });

  it("与收单无关的码不问", () => {
    render(withI18n(<FailureCard code="MODEL_MISSING" />));
    expect(screen.getByRole("button", { name: t("common:failure.action.open_models") })).toBeTruthy();
    expect(commandMock("billingAvailable")).not.toHaveBeenCalled();
  });

  it("账户被冻结（纵切 3 C3）：卖额度的服务器上也不给「升级」，去账户页看说明、不多问", () => {
    setCommandReply("billingAvailable", { status: "ok", data: true });
    const routes = captureNavigation();
    render(withI18n(<FailureCard code="ACCOUNT_FROZEN" />));
    expect(screen.getByText(failureMessage("ACCOUNT_FROZEN"))).toBeTruthy();
    const button = screen.getByRole("button", { name: t("common:failure.action.open_account") });
    expect(screen.queryByRole("button", { name: t("common:failure.action.upgrade") })).toBeNull();
    expect(commandMock("billingAvailable")).not.toHaveBeenCalled();
    fireEvent.click(button);
    expect(routes).toContain("settings://account/billing");
  });

  it("撞上自设的本月预付上限（纵切 3 C4）：卖额度的服务器上也不给「升级」，去上限控件、不多问", () => {
    setCommandReply("billingAvailable", { status: "ok", data: true });
    const routes = captureNavigation();
    render(withI18n(<FailureCard code="SPEND_CAP_REACHED" />));
    expect(screen.getByText(failureMessage("SPEND_CAP_REACHED"))).toBeTruthy();
    const button = screen.getByRole("button", { name: t("common:failure.action.open_spend_cap") });
    expect(screen.queryByRole("button", { name: t("common:failure.action.upgrade") })).toBeNull();
    expect(commandMock("billingAvailable")).not.toHaveBeenCalled();
    fireEvent.click(button);
    expect(routes).toContain("settings://account/spend_cap");
  });

  it("领不到免费额度（纵切 3 C5）：卖额度的服务器上也不给「升级」，去账户页的免费额度说明、不多问", () => {
    setCommandReply("billingAvailable", { status: "ok", data: true });
    const routes = captureNavigation();
    render(withI18n(<FailureCard code="FREE_QUOTA_WITHHELD" />));
    expect(screen.getByText(failureMessage("FREE_QUOTA_WITHHELD"))).toBeTruthy();
    const button = screen.getByRole("button", { name: t("common:failure.action.open_free_quota") });
    expect(screen.queryByRole("button", { name: t("common:failure.action.upgrade") })).toBeNull();
    expect(commandMock("billingAvailable")).not.toHaveBeenCalled();
    fireEvent.click(button);
    expect(routes).toContain("settings://account/free_quota");
  });

  it("HUD 回执与失败卡同一取法", async () => {
    setCommandReply("billingAvailable", { status: "ok", data: true });
    render(
      withI18n(<HudReceipt receipt={{ kind: "error", message: "x", code: "QUOTA_EXHAUSTED" }} />),
    );
    expect(await screen.findByRole("button", { name: t("common:failure.action.upgrade") })).toBeTruthy();
  });
});
