// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import { renderHook } from "@testing-library/react";

vi.mock("../ipc/bindings", () => import("../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble } from "../ipc/bindings-test-double";
import { usePanelTextEditing } from "./use-panel-text-editing";

afterEach(() => resetBindingsDouble());

describe("usePanelTextEditing · 浮层输入框的编辑态上报", () => {
  it("指针按下即上报开始编辑：非激活面板里 focus 要等面板被激活后才到（ADR-163 ①）", () => {
    const { result } = renderHook(() => usePanelTextEditing());
    result.current.onPointerDown();
    expect(commandMock("panelTextEditing")).toHaveBeenCalledWith(true);
  });

  it("按下与随后到达的 focus 只报一次，离开报一次", () => {
    const { result } = renderHook(() => usePanelTextEditing());
    result.current.onPointerDown();
    result.current.onFocus();
    result.current.onBlur();
    expect(commandMock("panelTextEditing").mock.calls).toEqual([[true], [false]]);
  });

  it("编辑中卸载视为离开", () => {
    const { result, unmount } = renderHook(() => usePanelTextEditing());
    result.current.onFocus();
    unmount();
    expect(commandMock("panelTextEditing").mock.calls).toEqual([[true], [false]]);
  });
});
