// @vitest-environment happy-dom
/**
 * HUD 回执线（ADR-137 V5）：成功回执淡出完即交还调用方清掉；失败回执常驻、修复动作
 * 没给就取失败注册表的深链（与 FailureCard 同一张表）。
 */

import { createElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { failureGuidance } from "../../i18n/failure";
import { NAVIGATE_EVENT } from "../../lib/navigate";
import { HudReceipt } from "./HudReceipt";

afterEach(cleanup);

describe("HudReceipt", () => {
  it("没有回执时什么也不画", () => {
    const { container } = render(withI18n(createElement(HudReceipt, { receipt: null })));
    expect(container.innerHTML).toBe("");
  });

  it("成功回执是 status，淡出动画播完才交还；子元素的动画不算", () => {
    const onExpire = vi.fn();
    const onOpen = vi.fn();
    render(
      withI18n(
        createElement(HudReceipt, {
          receipt: { kind: "success", message: "已存为速记", action: { label: "打开", onSelect: onOpen } },
          onExpire,
        }),
      ),
    );
    const line = screen.getByRole("status");
    fireEvent.animationEnd(line.querySelector(".luna-hud-receipt__icon")!);
    expect(onExpire).not.toHaveBeenCalled();
    fireEvent.animationEnd(line);
    expect(onExpire).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByText("打开"));
    expect(onOpen).toHaveBeenCalled();
  });

  it("失败回执是 alert、不交还；没给动作时取注册表的修复深链", () => {
    const onExpire = vi.fn();
    const seen: string[] = [];
    const onNavigate = (event: Event) => {
      seen.push((event as CustomEvent<string>).detail);
      event.preventDefault();
    };
    window.addEventListener(NAVIGATE_EVENT, onNavigate);
    render(
      withI18n(
        createElement(HudReceipt, {
          receipt: { kind: "error", message: "没有可用的模型", detail: "whisper small", code: "MODEL_MISSING" },
          onExpire,
        }),
      ),
    );
    const line = screen.getByRole("alert");
    fireEvent.animationEnd(line);
    expect(onExpire).not.toHaveBeenCalled();
    expect(screen.getByText("whisper small")).toBeTruthy();
    const guidance = failureGuidance("MODEL_MISSING")!;
    fireEvent.click(screen.getByText(t(guidance.labelKey)));
    expect(seen).toEqual([guidance.route]);
    window.removeEventListener(NAVIGATE_EVENT, onNavigate);
  });
});
