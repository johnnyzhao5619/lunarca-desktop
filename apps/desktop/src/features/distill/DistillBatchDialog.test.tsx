// @vitest-environment happy-dom
/**
 * 批量整理确认卡上的任务前预估（纵切 3 C4，PRD-13-040 ②）。
 *
 * 卡本来就是「开始之前把代价摆出来」的地方：约消耗多少、从哪本账扣写在卡上；不够时开始之前就说清
 * 并给修复按钮，开始按钮不可点——而不是跑到一半失败。Rust 说不需要预估（本机跑 / 未登录）时卡上
 * 一个字都不多，照常可以开始。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
vi.mock("../../ipc/cloud-bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { NAVIGATE_EVENT } from "../../lib/navigate";
import { DistillBatchDialog } from "./DistillBatchDialog";
import type { DistillBatchController } from "./use-distill-batch";

function controller(): DistillBatchController {
  return {
    dialog: {
      kind: "ready",
      scope: null,
      startError: null,
      preflight: { pending: 3, fresh: 0, edited: 0, empty: 0, running: false },
    },
    openDialog: vi.fn(),
    closeDialog: vi.fn(),
    start: vi.fn(async () => undefined),
    outcome: null,
    outcomeError: null,
    dismissQueued: vi.fn(),
  };
}

function startButton(): HTMLButtonElement {
  return screen.getByRole("button", { name: t("workspace:distill.batchStart", { count: 3 }) }) as HTMLButtonElement;
}

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("distillTemplateChoices", { status: "ok", data: [] });
});
afterEach(cleanup);

describe("批量整理卡上的预估", () => {
  it("不需要预估：卡上不多说，照常可以开始；问的是这个范围", async () => {
    setCommandReply("taskEstimate", null);
    render(withI18n(<DistillBatchDialog controller={controller()} />));
    await waitFor(() =>
      expect(commandMock("taskEstimate")).toHaveBeenCalledWith({
        kind: "distill_scope",
        scope: null,
        full_text: null,
      }),
    );
    expect(screen.queryByText(t("cloud:estimate.note"))).toBeNull();
    expect(startButton().disabled).toBe(false);
  });

  it("从预付扣：写出约消耗与约多少 credits，并说明以服务端计量为准", async () => {
    setCommandReply("taskEstimate", {
      amounts: [{ unit: "dimension", dimension: "tokens_out", amount: 9000 }],
      outcome: { state: "prepaid", credits: 3 },
    });
    render(withI18n(<DistillBatchDialog controller={controller()} />));
    await screen.findByText(t("cloud:estimate.note"));
    expect(
      screen.getByText(t("cloud:estimate.from_prepaid", { credits: "3" }), { exact: false }),
    ).toBeTruthy();
    expect(startButton().disabled).toBe(false);
  });

  it("撞上自设上限：开始之前说清、开始不可点，修复按钮关卡并去上限控件", async () => {
    const routes: string[] = [];
    const onNavigate = (event: Event) => {
      routes.push((event as CustomEvent<string>).detail);
      event.preventDefault();
    };
    window.addEventListener(NAVIGATE_EVENT, onNavigate);
    setCommandReply("taskEstimate", { amounts: [], outcome: { state: "insufficient", reason: "spend_cap" } });
    const ctl = controller();
    render(withI18n(<DistillBatchDialog controller={ctl} />));
    await screen.findByText(t("cloud:estimate.short.spend_cap.body"));
    expect(startButton().disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: t("common:failure.action.open_spend_cap") }));
    expect(ctl.closeDialog).toHaveBeenCalled();
    expect(routes).toEqual(["settings://account/spend_cap"]);
    window.removeEventListener(NAVIGATE_EVENT, onNavigate);
  });
});
