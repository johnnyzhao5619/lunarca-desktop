// @vitest-environment happy-dom
/**
 * 「开始」之前的那道门（纵切 3 C4，PRD-13-040 ②）：只在预估说不够时拦下并说清原因与下一步；
 * 够、不需要、问不到（含命令抛错）一律放行——预估是善意提醒，服务端预留才是真正的闸。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../ipc/bindings", () => import("../ipc/bindings-test-double"));
vi.mock("../ipc/cloud-bindings", () => import("../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../ipc/bindings-test-double";
import type { ConfirmOptions } from "../components/ConfirmHost";
import { i18n } from "../i18n/i18n";
import { NAVIGATE_EVENT } from "../lib/navigate";
import { clearToStart } from "./use-task-start-gate";

const TASK = { kind: "retranscribe", note_rel: "Recordings/会议.md" } as const;

function captureNavigation(): string[] {
  const routes: string[] = [];
  const onNavigate = (event: Event) => {
    routes.push((event as CustomEvent<string>).detail);
    event.preventDefault();
  };
  window.addEventListener(NAVIGATE_EVENT, onNavigate);
  afterEach(() => window.removeEventListener(NAVIGATE_EVENT, onNavigate));
  return routes;
}

beforeEach(() => resetBindingsDouble());

describe("任务前预估的门", () => {
  it("不需要预估（本机跑 / 未登录：Rust 回 null）与够用时一声不响地放行", async () => {
    const ask = vi.fn();
    setCommandReply("taskEstimate", null);
    expect(await clearToStart(TASK, ask)).toBe(true);
    setCommandReply("taskEstimate", {
      amounts: [{ unit: "minutes", minutes: 30 }],
      outcome: { state: "prepaid", credits: 4 },
    });
    expect(await clearToStart(TASK, ask)).toBe(true);
    expect(ask).not.toHaveBeenCalled();
    expect(commandMock("taskEstimate")).toHaveBeenCalledWith(TASK);
  });

  it("问不到（命令抛错）绝不挡路", async () => {
    setCommandReply("taskEstimate", () => {
      throw new Error("ipc down");
    });
    const ask = vi.fn();
    expect(await clearToStart(TASK, ask)).toBe(true);
    expect(ask).not.toHaveBeenCalled();
  });

  it("撞上自设上限：拦下、说清，修复按钮是「调整预付上限」并落在上限控件上——不是「购买」", async () => {
    const routes = captureNavigation();
    setCommandReply("taskEstimate", { amounts: [], outcome: { state: "insufficient", reason: "spend_cap" } });
    setCommandReply("billingAvailable", { status: "ok", data: true });
    let seen: ConfirmOptions | null = null;
    const ask = vi.fn(async (options: ConfirmOptions) => {
      seen = options;
      return true;
    });
    expect(await clearToStart(TASK, ask)).toBe(false);
    expect(seen).toMatchObject({
      title: i18n.t("cloud:estimate.short.spend_cap.title"),
      description: i18n.t("cloud:estimate.short.spend_cap.body"),
      confirmLabel: i18n.t("common:failure.action.open_spend_cap"),
      destructive: false,
    });
    expect(routes).toEqual(["settings://account/spend_cap"]);
    expect(commandMock("billingAvailable"), "上限这一支与卖不卖无关，不该多问一次").not.toHaveBeenCalled();
  });

  it("额度不够：卖额度的服务器上给「升级或购买额度」；用户取消就哪儿也不去", async () => {
    const routes = captureNavigation();
    setCommandReply("taskEstimate", { amounts: [], outcome: { state: "insufficient", reason: "quota" } });
    setCommandReply("billingAvailable", { status: "ok", data: true });
    const ask = vi.fn(async () => false);
    expect(await clearToStart(TASK, ask)).toBe(false);
    expect(ask).toHaveBeenCalledWith(
      expect.objectContaining({
        title: i18n.t("cloud:estimate.short.quota.title"),
        confirmLabel: i18n.t("common:failure.action.upgrade"),
      }),
    );
    expect(routes).toEqual([]);
  });
});
