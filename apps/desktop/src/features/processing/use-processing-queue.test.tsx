// @vitest-environment happy-dom
/**
 * 处理队列的共享订阅：消费者按选择器取值，选出来的值没变就不重渲染（2026-10-15 真机卡顿）。
 */
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import type { ProcTaskDto } from "../../ipc/bindings";
import {
  emitEvent,
  listenerCount,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";
import { activeCount, useNoteTasks, useProcessingQueueSelector } from "./use-processing-queue";

function running(id: string, noteRel: string, done: number): ProcTaskDto {
  return {
    id,
    kind: "merge",
    label: id,
    note_rel: noteRel,
    status: { state: "running", data: { step: "transcribing", done, total: 100 } },
    executed_by: null,
    queue: null,
    restored: false,
  } as ProcTaskDto;
}

afterEach(cleanup);
beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("processingQueueSnapshot", []);
});

describe("useProcessingQueueSelector", () => {
  it("别的篇推进时这一篇的任务不重渲染；这一篇推进时才渲染", async () => {
    let renders = 0;
    const view = renderHook(() => {
      renders += 1;
      return useNoteTasks("a.md");
    });
    await waitFor(() => expect(listenerCount("processingQueue")).toBe(1));
    act(() => emitEvent("processingQueue", { items: [running("1", "a.md", 10)] }));
    expect(view.result.current).toHaveLength(1);
    const before = renders;

    act(() =>
      emitEvent("processingQueue", { items: [running("1", "a.md", 10), running("2", "b.md", 5)] }),
    );
    act(() =>
      emitEvent("processingQueue", { items: [running("1", "a.md", 10), running("2", "b.md", 60)] }),
    );
    expect(renders).toBe(before);

    act(() =>
      emitEvent("processingQueue", { items: [running("1", "a.md", 50), running("2", "b.md", 60)] }),
    );
    expect(renders).toBe(before + 1);
    expect(view.result.current[0]?.status).toEqual({
      state: "running",
      data: { step: "transcribing", done: 50, total: 100 },
    });
  });

  it("只取角标数：进度推进不渲染，条数变了才渲染", async () => {
    let renders = 0;
    const view = renderHook(() => {
      renders += 1;
      return useProcessingQueueSelector((state) => activeCount(state.items));
    });
    await waitFor(() => expect(listenerCount("processingQueue")).toBe(1));
    act(() => emitEvent("processingQueue", { items: [running("1", "a.md", 10)] }));
    const before = renders;
    act(() => emitEvent("processingQueue", { items: [running("1", "a.md", 70)] }));
    expect(renders).toBe(before);
    act(() =>
      emitEvent("processingQueue", { items: [running("1", "a.md", 80), running("2", "b.md", 0)] }),
    );
    expect(view.result.current).toBe(2);
    expect(renders).toBe(before + 1);
  });

  it("晚回来的启动快照不覆盖已到的事件；全部卸载后撤掉监听、回到「还没读回来」", async () => {
    let answer: (items: ProcTaskDto[]) => void = () => {};
    setCommandReply(
      "processingQueueSnapshot",
      () => new Promise<ProcTaskDto[]>((resolve) => (answer = resolve)),
    );
    const first = renderHook(() => useProcessingQueueSelector((state) => state));
    await waitFor(() => expect(listenerCount("processingQueue")).toBe(1));
    act(() => emitEvent("processingQueue", { items: [running("1", "a.md", 10)] }));
    await act(async () => answer([]));
    expect(first.result.current.items).toHaveLength(1);
    expect(first.result.current.loaded).toBe(true);

    first.unmount();
    await waitFor(() => expect(listenerCount("processingQueue")).toBe(0));
    setCommandReply("processingQueueSnapshot", () => new Promise<ProcTaskDto[]>(() => {}));
    const second = renderHook(() => useProcessingQueueSelector((state) => state.loaded));
    expect(second.result.current).toBe(false);
  });
});
