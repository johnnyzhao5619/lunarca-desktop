// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import { renderHook } from "@testing-library/react";
import { emitEvent, resetBindingsDouble } from "../ipc/bindings-test-double";
import { useVaultReload } from "./use-vault-reload";

vi.mock("../ipc/bindings", () => import("../ipc/bindings-test-double"));

afterEach(() => {
  resetBindingsDouble();
  vi.useRealTimers();
});

describe("useVaultReload", () => {
  it("一阵 vault://changed 合并成一次重取", () => {
    vi.useFakeTimers();
    const reload = vi.fn();
    renderHook(() => useVaultReload(reload));
    emitEvent("vaultChanged", { rel_path: "a.md", deleted: false });
    emitEvent("vaultChanged", { rel_path: "b.md", deleted: false });
    vi.advanceTimersByTime(500);
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it("索引扫描做完、确有篇数变了时也重取（V1b）；进行中的帧与什么都没变的终态不重取", () => {
    vi.useFakeTimers();
    const reload = vi.fn();
    renderHook(() => useVaultReload(reload));
    emitEvent("scanProgress", { indexed: 40, total: 100, current_path: "x.md", done: false, cancelled: false });
    emitEvent("scanProgress", { indexed: 0, total: 0, current_path: null, done: true, cancelled: false });
    vi.advanceTimersByTime(500);
    expect(reload).not.toHaveBeenCalled();
    emitEvent("scanProgress", { indexed: 100, total: 100, current_path: null, done: true, cancelled: false });
    vi.advanceTimersByTime(500);
    expect(reload).toHaveBeenCalledTimes(1);
  });
});
