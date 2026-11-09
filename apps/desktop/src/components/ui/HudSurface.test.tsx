// @vitest-environment happy-dom
/**
 * 悬浮窗外壳的 Esc 逃生通道（ADR-111 ⑤ ⑵）：普通 Esc 关窗；输入法组字中、以及 WKWebView 上
 * 组字刚结束的那一下 Esc 属于输入法（判据 `@lunarca/editor-core/ime-composition`），原样放行。
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";
import { useTranslation } from "react-i18next";

vi.mock("../../hooks/use-window-drag", () => ({
  useWindowDrag: () => ({ onPointerDown: vi.fn() }),
}));

import { HudSurface } from "./HudSurface";
import { withI18n } from "../../i18n/i18n-test-fixture";

afterEach(cleanup);

function TestHud({ onDismiss }: { onDismiss: () => void }) {
  const { t } = useTranslation("common");
  return (
    <HudSurface className="test-hud" aria-label={t("hud.action.close")} onDismiss={onDismiss}>
      {t("hud.action.close")}
    </HudSurface>
  );
}

describe("HudSurface 键盘退出", () => {
  it("普通 Escape 关闭 HUD", () => {
    const onDismiss = vi.fn();
    render(withI18n(<TestHud onDismiss={onDismiss} />));
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onDismiss).toHaveBeenCalledOnce();
  });

  it("WKWebView 上组字刚结束的那一下 Escape 也交给输入法；之后再按照常关闭", () => {
    const onDismiss = vi.fn();
    render(withI18n(<TestHud onDismiss={onDismiss} />));
    // compositionend 先到、keydown 后到，且 isComposing 已为 false（2026-09-09 真机事实）。
    window.dispatchEvent(new Event("compositionstart"));
    const end = new Event("compositionend");
    window.dispatchEvent(end);
    const sameKey = new KeyboardEvent("keydown", { key: "Escape" });
    Object.defineProperty(sameKey, "timeStamp", { value: end.timeStamp + 5 });
    window.dispatchEvent(sameKey);
    expect(onDismiss).not.toHaveBeenCalled();
    const later = new KeyboardEvent("keydown", { key: "Escape" });
    Object.defineProperty(later, "timeStamp", { value: end.timeStamp + 500 });
    window.dispatchEvent(later);
    expect(onDismiss).toHaveBeenCalledOnce();
  });

  it("输入法组字期间的 Escape 只交给输入法", () => {
    const onDismiss = vi.fn();
    render(withI18n(<TestHud onDismiss={onDismiss} />));
    fireEvent.keyDown(window, { key: "Escape", isComposing: true, keyCode: 229 });
    expect(onDismiss).not.toHaveBeenCalled();
  });
});
