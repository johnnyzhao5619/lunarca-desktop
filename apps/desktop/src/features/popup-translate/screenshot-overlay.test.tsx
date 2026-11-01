// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import {
  commandMock,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { ScreenshotOverlay, moveRect, resizeRect } from "./ScreenshotOverlay";

beforeEach(() => {
  resetBindingsDouble();
  HTMLElement.prototype.setPointerCapture = vi.fn();
});

afterEach(cleanup);

function selectArea(element: HTMLElement) {
  fireEvent.pointerDown(element, { button: 0, pointerId: 1, clientX: 10, clientY: 20 });
  fireEvent.pointerMove(element, { button: 0, pointerId: 1, clientX: 110, clientY: 90 });
  fireEvent.pointerUp(element, { button: 0, pointerId: 1, clientX: 110, clientY: 90 });
}

/** 松手只进入可调整态；点工具条上的「翻译」才提交。 */
function selectAndConfirm(view: ReturnType<typeof render>, element: HTMLElement) {
  selectArea(element);
  fireEvent.click(view.getByText(t("popup:overlay.translate")));
}

describe("截屏框选失败恢复", () => {
  it("提交失败后显示原因并允许重新框选", async () => {
    let attempts = 0;
    setCommandReply("screenshotOverlayConfirm", () => {
      attempts += 1;
      return attempts === 1
        ? { status: "error", error: "SCREEN_CAPTURE_FAILED: denied" }
        : { status: "ok", data: null };
    });
    const view = render(withI18n(<ScreenshotOverlay />));
    const overlay = view.container.querySelector<HTMLElement>(".luna-shot")!;

    selectAndConfirm(view, overlay);
    await waitFor(() => expect(view.getByRole("alert")).toBeTruthy());
    expect(commandMock("screenshotOverlayConfirm")).toHaveBeenCalledTimes(1);

    selectAndConfirm(view, overlay);
    await waitFor(() => expect(commandMock("screenshotOverlayConfirm")).toHaveBeenCalledTimes(2));
  });
});

describe("截屏框选的可调整态（ADR-137 H4）", () => {
  it("松手不提交：先给把手与工具条，双击框内才确认", async () => {
    setCommandReply("screenshotOverlayConfirm", { status: "ok", data: null });
    const view = render(withI18n(<ScreenshotOverlay />));
    const overlay = view.container.querySelector<HTMLElement>(".luna-shot")!;
    selectArea(overlay);
    expect(commandMock("screenshotOverlayConfirm")).not.toHaveBeenCalled();
    expect(view.container.querySelectorAll("[data-handle]")).toHaveLength(8);
    fireEvent.doubleClick(view.container.querySelector(".luna-shot__selection")!);
    await waitFor(() =>
      expect(commandMock("screenshotOverlayConfirm")).toHaveBeenCalledWith(10, 20, 100, 70),
    );
  });

  it("把手只动它管的边，越过对边时翻转；整体移动不出视口", () => {
    const base = { x: 10, y: 10, width: 100, height: 50 };
    expect(resizeRect(base, "r", { x: 200, y: 999 })).toEqual({ x: 10, y: 10, width: 190, height: 50 });
    expect(resizeRect(base, "tl", { x: 0, y: 0 })).toEqual({ x: 0, y: 0, width: 110, height: 60 });
    expect(resizeRect(base, "l", { x: 150, y: 0 })).toEqual({ x: 110, y: 10, width: 40, height: 50 });
    expect(moveRect(base, { x: 0, y: 0 }, { x: -50, y: -50 })).toEqual({ ...base, x: 0, y: 0 });
  });
});
