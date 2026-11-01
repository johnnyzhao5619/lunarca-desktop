// @vitest-environment happy-dom
/**
 * 文件树上只在云端的文件（ADR-156）：下载到本机 / 释放本机空间的结果各说一句话，下载进度随事件
 * 进出；「释放本机空间」只在当前库开着同步时给。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import {
  commandMock,
  emitEvent,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { useDownloadPercent, useSyncCloudOnly } from "./use-sync-cloud-only";

function overview(phase: string) {
  return {
    gate: "ready",
    paused: null,
    storage: null,
    read_only: null,
    vaults: [{ path: "/v/a", is_open: true, phase }],
  };
}

function mount(onStatus = vi.fn()) {
  const view = renderHook(() => useSyncCloudOnly(onStatus), {
    wrapper: ({ children }: { children: ReactNode }) => withI18n(<>{children}</>),
  });
  return { view, onStatus };
}

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("syncOverview", { status: "ok", data: overview("idle") });
});
afterEach(() => cleanup());

describe("useSyncCloudOnly", () => {
  it("下载：下好了返回 true；云端没这一版、失败各说一句；进度随事件进出", async () => {
    const { view, onStatus } = mount();
    act(() => emitEvent("syncDownloadProgress", { path: "r.wav", done: 3, total: 12 }));
    expect(view.result.current.progress.get("r.wav")).toBe(25);
    act(() => emitEvent("syncDownloadProgress", { path: "r.wav", done: 12, total: 12 }));
    expect(view.result.current.progress.get("r.wav")).toBeUndefined();

    setCommandReply("syncMaterialize", { status: "ok", data: "done" });
    await expect(view.result.current.download("r.wav", "r.wav")).resolves.toBe(true);
    expect(commandMock("syncMaterialize")).toHaveBeenCalledWith("r.wav");

    setCommandReply("syncMaterialize", { status: "ok", data: "not_found" });
    await expect(view.result.current.download("r.wav", "r.wav")).resolves.toBe(false);
    expect(onStatus).toHaveBeenLastCalledWith(t("workspace:cloud.sync_missing", { name: "r.wav" }));

    setCommandReply("syncMaterialize", { status: "error", error: "sync_signed_out" });
    await expect(view.result.current.download("r.wav", "r.wav")).resolves.toBe(false);
    expect(onStatus).toHaveBeenLastCalledWith(t("workspace:cloud.sync_failed", { name: "r.wav" }));
  });

  it("释放：没同步完的不放、云端还没有的不放，各说清楚", async () => {
    const { view, onStatus } = mount();
    await waitFor(() => expect(view.result.current.synced).toBe(true));
    for (const [data, key] of [
      ["done", "cloud.sync_evicted"],
      ["changed", "cloud.sync_evict_unsynced"],
      ["not_found", "cloud.sync_evict_not_synced"],
    ] as const) {
      setCommandReply("syncEvict", { status: "ok", data });
      await view.result.current.evict("r.wav", "r.wav");
      expect(onStatus).toHaveBeenLastCalledWith(t(`workspace:${key}`, { name: "r.wav" }));
    }
  });

  it("当前库没开同步：不给释放本机空间", async () => {
    setCommandReply("syncOverview", { status: "ok", data: overview("off") });
    const { view } = mount();
    await waitFor(() => expect(commandMock("syncOverview")).toHaveBeenCalled());
    expect(view.result.current.synced).toBe(false);
  });

  it("整棵树不随同步轮次与下载进度重渲染：只在「开没开同步」翻转时，进度只到那一行", async () => {
    let treeRenders = 0;
    const tree = renderHook(
      () => {
        treeRenders += 1;
        return useSyncCloudOnly(vi.fn());
      },
      { wrapper: ({ children }: { children: ReactNode }) => withI18n(<>{children}</>) },
    );
    await waitFor(() => expect(tree.result.current.synced).toBe(true));
    let rowRenders = 0;
    const row = renderHook(() => {
      rowRenders += 1;
      return useDownloadPercent(tree.result.current.progress, "r.wav");
    });
    const treeBefore = treeRenders;
    const rowBefore = rowRenders;

    // 同一轮里只有每库细节在变（阶段、字节数）：树要的那一位没变，不重渲染。
    act(() =>
      emitEvent("syncStatus", {
        ...overview("syncing"),
        vaults: [{ path: "/v/a", is_open: true, phase: "syncing", local_bytes: 42 }],
      }),
    );
    act(() => emitEvent("syncStatus", overview("idle")));
    // 别的文件的下载进度：这一行不动。
    act(() => emitEvent("syncDownloadProgress", { path: "other.wav", done: 1, total: 4 }));
    expect(treeRenders).toBe(treeBefore);
    expect(rowRenders).toBe(rowBefore);

    // 这一行的进度：只有这一行重渲染。
    act(() => emitEvent("syncDownloadProgress", { path: "r.wav", done: 1, total: 4 }));
    expect(row.result.current).toBe(25);
    expect(rowRenders).toBe(rowBefore + 1);
    expect(treeRenders).toBe(treeBefore);

    // 关掉同步：那一位翻转，树才重渲染。
    act(() => emitEvent("syncStatus", overview("off")));
    expect(tree.result.current.synced).toBe(false);
    expect(treeRenders).toBe(treeBefore + 1);
  });
});
