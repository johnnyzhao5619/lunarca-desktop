// @vitest-environment happy-dom
/**
 * 「重置应用」的失败出口（PRD-09-026，TDD-13 §5）：成功时 Rust 保存、重置后直接重启，界面只处理失败。
 * 被拦下时要说清原因、按钮恢复可点；其余错误照旧按 Vault 错误显示。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
vi.mock("../../components/ConfirmHost", () => ({ useConfirm: () => async () => true }));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import en from "../../i18n/locales/en-US/settings.json";
import zh from "../../i18n/locales/zh-CN/settings.json";
import { DataFootprintPanel } from "./DataFootprintPanel";
import { RESTART_BLOCK_CODES } from "./restart-block-codes";

function mount() {
  setCommandReply("dataFootprint", { status: "ok", data: { entries: [] } });
  return render(withI18n(createElement(DataFootprintPanel)));
}

function resetButton() {
  return screen.getByRole("button", { name: t("settings:footprint.action.reset") }) as HTMLButtonElement;
}

beforeEach(resetBindingsDouble);
afterEach(cleanup);

describe("重置应用", () => {
  it.each(RESTART_BLOCK_CODES)("%s：说明原因，按钮恢复可点", async (code) => {
    setCommandReply("resetApp", { status: "error", error: code });
    mount();
    fireEvent.click(resetButton());
    await waitFor(() => expect(commandMock("resetApp")).toHaveBeenCalledTimes(1));
    expect(await screen.findByText(t(`settings:footprint.reset_blocked.${code}`))).toBeTruthy();
    expect(resetButton().disabled).toBe(false);
  });

  it("等 Rust 结果期间按钮禁用：成功时进程直接换掉，不会再回到可点", async () => {
    setCommandReply("resetApp", new Promise(() => {}));
    mount();
    fireEvent.click(resetButton());
    await waitFor(() => expect(resetButton().disabled).toBe(true));
  });

  it("每个拦下原因的文案中英都在", () => {
    for (const code of RESTART_BLOCK_CODES) {
      expect(zh.footprint.reset_blocked[code], code).toBeTruthy();
      expect(en.footprint.reset_blocked[code], code).toBeTruthy();
    }
  });
});
