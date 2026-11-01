import { describe, expect, it } from "vitest";
import {
  applyFloatingMenuEvent,
  type FloatingMenuResult,
} from "./use-floating-menu";

function result(output = ""): FloatingMenuResult {
  return {
    status: "idle",
    output,
    message: null,
    action: "summarize",
    truncatedChars: null,
    meta: null,
    chunkProgress: null,
  };
}

describe("floating menu projections", () => {
  it("accumulates streamed tokens and replaces output on done", () => {
    const streamed = applyFloatingMenuEvent(result("hello"), {
      kind: "token",
      data: " world",
    });
    expect(streamed).toMatchObject({
      status: "running",
      output: "hello world",
    });

    expect(
      applyFloatingMenuEvent(streamed, {
        kind: "done",
        data: "final",
      }),
    ).toMatchObject({
      status: "done",
      output: "final",
    });
  });

  it("keeps the truncation notice across the streaming lifecycle", () => {
    const truncated = applyFloatingMenuEvent(result(), {
      kind: "truncated",
      data: 12000,
    });
    expect(truncated.truncatedChars).toBe(12000);

    const streamed = applyFloatingMenuEvent(truncated, {
      kind: "token",
      data: "partial",
    });
    expect(streamed.truncatedChars).toBe(12000);

    const done = applyFloatingMenuEvent(streamed, {
      kind: "done",
      data: "final",
    });
    expect(done.truncatedChars).toBe(12000);
  });

  it("keeps the round facts (landing / route / sent chars) across the stream", () => {
    const withMeta = applyFloatingMenuEvent(result(), {
      kind: "floating_menu",
      data: { suggested_landing: "replace", on_server: false, sent_chars: 42, context_chars: 20, chunks: 0 },
    });
    expect(withMeta.meta).toEqual({
      suggested_landing: "replace",
      on_server: false,
      sent_chars: 42,
      context_chars: 20,
      chunks: 0,
    });
    const started = applyFloatingMenuEvent(withMeta, { kind: "started" });
    const done = applyFloatingMenuEvent(started, { kind: "done", data: "final" });
    expect(done.meta?.suggested_landing).toBe("replace");
    expect(done.status).toBe("done");
  });

  it("reports chunked-summary progress as a running state", () => {
    const progressed = applyFloatingMenuEvent(result(), {
      kind: "floating_chunk",
      data: { done: 1, total: 3 },
    });
    expect(progressed).toMatchObject({ status: "running", chunkProgress: { done: 1, total: 3 } });
    // 分段跑完转入汇总那一轮的流式 token，进度不再顶替状态。
    const streamed = applyFloatingMenuEvent(progressed, { kind: "token", data: "abc" });
    expect(streamed.chunkProgress).toEqual({ done: 1, total: 3 });
    expect(streamed.output).toBe("abc");
  });

  it("projects cancellation as a terminal state that keeps produced output", () => {
    const streamed = applyFloatingMenuEvent(result(), {
      kind: "token",
      data: "partial",
    });
    const cancelled = applyFloatingMenuEvent(streamed, { kind: "cancelled" });
    expect(cancelled.status).toBe("cancelled");
    expect(cancelled.output).toBe("partial");
  });
});

describe("表格 AI 的结果不能落地（对标方案 T6）", () => {
  it("floating_rejected 按失败呈现、给出人话说明，不留输出", () => {
    const rejected = applyFloatingMenuEvent({ ...result(""), status: "running" }, { kind: "floating_rejected", data: "not_a_table" });
    expect(rejected.status).toBe("failed");
    expect(rejected.message).toBeTruthy();
    expect(rejected.message).not.toBe("not_a_table");
  });
});
