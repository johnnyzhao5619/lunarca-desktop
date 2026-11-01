// @vitest-environment happy-dom

import { StrictMode } from "react";
import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { recordWorkspaceMetric } = vi.hoisted(() => ({
  recordWorkspaceMetric: vi.fn(async () => ({ status: "ok", data: null })),
}));

vi.mock("../ipc/bindings", () => ({
  commands: { recordWorkspaceMetric },
}));

import { useStatusVisitMetrics } from "./use-status-visit-metrics";

describe("状态面访问终结语义", () => {
  beforeEach(() => {
    recordWorkspaceMetric.mockClear();
  });

  it("StrictMode 重放与组件卸载都不把在途访问伪装成已终结", () => {
    const { unmount } = renderHook(() => useStatusVisitMetrics("notifications", true), {
      wrapper: StrictMode,
    });
    expect(recordWorkspaceMetric).not.toHaveBeenCalled();
    unmount();
    expect(recordWorkspaceMetric).not.toHaveBeenCalled();
  });

  it("只在显式 open → closed 时上报一次，并保留交互事实", () => {
    const { result, rerender, unmount } = renderHook(
      ({ open }) => useStatusVisitMetrics("queue", open),
      { initialProps: { open: true }, wrapper: StrictMode },
    );
    act(() => result.current.markInteracted());
    rerender({ open: false });
    expect(recordWorkspaceMetric).toHaveBeenCalledTimes(1);
    expect(recordWorkspaceMetric).toHaveBeenCalledWith(
      expect.objectContaining({
        kind: "status_visit_finished",
        surface: "queue",
        interacted: true,
      }),
    );
    unmount();
    expect(recordWorkspaceMetric).toHaveBeenCalledTimes(1);
  });
});
