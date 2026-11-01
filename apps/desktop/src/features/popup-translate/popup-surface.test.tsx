// @vitest-environment happy-dom
/**
 * 弹窗翻译的表面裁决（ADR-137 H4）：输入模式可固定；有原文选区时「替换原文」是唯一主动作。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null;
  },
}));
vi.mock("../../hooks/use-hud-window-auto-grow", () => ({ useHudWindowAutoGrow: () => {} }));
vi.mock("../../hooks/use-window-drag", () => ({
  useWindowDrag: () => ({ onPointerDown: vi.fn() }),
}));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { PopupTranslateView } from "./PopupTranslateView";
import { ResultActionBar } from "./ResultActionBar";

const PRESETS = [{ id: "to_en", name: "英文", group: "target_lang" }];

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("popupSessionState", {
    status: "ok",
    data: { mode: "input", has_source: false, source_text: null, save_to_translate_space: false, draft: null },
  });
});
afterEach(cleanup);

describe("弹窗翻译表面", () => {
  it("输入模式可固定：图钉走 Rust 的 popup_set_pinned，按下态随返回值", async () => {
    setCommandReply("popupSetPinned", (pinned: boolean) => ({ status: "ok", data: pinned }));
    const view = render(withI18n(createElement(PopupTranslateView, { presets: PRESETS })));
    const pin = await view.findByRole("button", { name: t("popup:action.pin") });
    fireEvent.click(pin);
    await waitFor(() => expect(commandMock("popupSetPinned")).toHaveBeenCalledWith(true));
    expect(await view.findByRole("button", { name: t("popup:action.unpin") })).toBeTruthy();
  });

  it("有原文选区时「替换原文」是主按钮，复制退为次级；没有选区时复制是主按钮", () => {
    const props = {
      canUseResult: true,
      canSaveNote: true,
      onCopy: vi.fn(),
      onRegenerate: vi.fn(),
      onReplace: vi.fn(),
      onSaveNote: vi.fn(),
      speakText: "hi",
      speakLang: null,
      speakScope: "test",
    };
    const view = render(withI18n(createElement(ResultActionBar, { ...props, showReplace: true })));
    const replace = view.getByText(t("popup:action.replace")).closest("button")!;
    const copy = view.getByText(t("popup:action.copy")).closest("button")!;
    expect(replace.className).toContain("luna-btn--primary");
    expect(copy.className).toContain("luna-btn--secondary");
    cleanup();
    const solo = render(withI18n(createElement(ResultActionBar, { ...props, showReplace: false })));
    expect(solo.getByText(t("popup:action.copy")).closest("button")!.className).toContain("luna-btn--primary");
  });
});
