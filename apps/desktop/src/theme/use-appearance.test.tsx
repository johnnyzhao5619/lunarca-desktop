// @vitest-environment happy-dom
/**
 * 外观在运行中跟着主题目录变（纵切 7a T3，2026-10-21 真机）：服务端封禁了在用的那套主题，设置不变，Rust 的
 * `appearance_theme` 已回落 Lunarca——外观必须随 `themes://changed` 重读，而不是等到下一次重启。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, waitFor } from "@testing-library/react";

vi.mock("../ipc/bindings", () => import("../ipc/bindings-test-double"));

import { commandMock, emitEvent, resetBindingsDouble, setCommandReply } from "../ipc/bindings-test-double";
import { useAppearance } from "./use-appearance";

function Probe() {
  useAppearance([], false);
  return null;
}

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("appearanceTheme", { family: "com.example.dusk", css: "@layer tokens.family{}" });
  setCommandReply("listSettings", { status: "ok", data: [] });
});
afterEach(cleanup);

describe("useAppearance", () => {
  it("主题目录换了一份（封禁 / 解除封禁）就重读在用的主题", async () => {
    render(<Probe />);
    await waitFor(() => expect(document.documentElement.dataset.themeFamily).toBe("com.example.dusk"));
    setCommandReply("appearanceTheme", { family: "luna", css: null });
    emitEvent("themesChanged", null);
    await waitFor(() => expect(document.documentElement.dataset.themeFamily).toBe("luna"));
    expect(commandMock("appearanceTheme")).toHaveBeenCalledTimes(2);
  });
});
