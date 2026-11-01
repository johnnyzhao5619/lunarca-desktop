// @vitest-environment happy-dom
/**
 * 选择面底部的「获取更多模板」（纵切 7b，PRD-18-004 / 006）：在线模板目录不可用时不出现；可用时去「应用 › 模板」，
 * 并先让所在的对话框关上。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../ipc/bindings", () => import("../ipc/bindings-test-double"));

import { resetBindingsDouble, setCommandReply } from "../ipc/bindings-test-double";
import { t, withI18n } from "../i18n/i18n-test-fixture";
import { NAVIGATE_EVENT } from "../lib/navigate";
import { GetMoreTemplates } from "./GetMoreTemplates";

beforeEach(() => resetBindingsDouble());
afterEach(cleanup);

describe("GetMoreTemplates", () => {
  it("目录不可用（Rust 回 null）时不放这个入口", async () => {
    setCommandReply("templateMarket", null);
    const { container } = render(withI18n(<GetMoreTemplates />));
    await waitFor(() => expect(container.textContent).toBe(""));
  });

  it("可用时点它：先关对话框，再去「应用 › 模板」", async () => {
    setCommandReply("templateMarket", { packs: [] });
    const routes: string[] = [];
    const listener = (event: Event) => {
      routes.push(String((event as CustomEvent).detail));
      event.preventDefault();
    };
    window.addEventListener(NAVIGATE_EVENT, listener);
    const onNavigate = vi.fn();
    render(withI18n(<GetMoreTemplates onNavigate={onNavigate} />));
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:apps.templates.getMore") }));
    expect(onNavigate).toHaveBeenCalledTimes(1);
    expect(routes).toEqual(["apps://templates"]);
    window.removeEventListener(NAVIGATE_EVENT, listener);
  });
});
