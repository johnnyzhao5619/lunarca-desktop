import { describe, expect, it } from "vitest";
import type { UsageRecordDto } from "../../ipc/bindings";
import { terminalStatusKind, toUsageRows } from "./use-usage";

function usageRecord(overrides: Partial<UsageRecordDto> = {}): UsageRecordDto {
  return {
    job_id: "job-1",
    origin: "floating_menu",
    capability: "translate",
    provider_id: "local-llm",
    provider_location: "local_service",
    model_id: "qwen3",
    audio_seconds: null,
    tokens_in: 12,
    tokens_out: 8,
    est_cost_minor: null,
    terminal_status: "finished",
    output_truncated: null,
    service_class: "batch",
    error_code: null,
    finished_at_ms: "1000",
    ...overrides,
  };
}

describe("用量与执行记录的投影（PRD-10-051）", () => {
  it("renders only metering metadata", () => {
    const rows = toUsageRows([usageRecord()]);
    const rendered = JSON.stringify(rows);

    expect(rendered).toContain("floating_menu");
    expect(rendered).toContain("local-llm");
    expect(rendered).not.toContain("private note body");
    expect(rendered).not.toContain("clipboard");
    expect(rendered).not.toContain("transcript");
  });

  it("carries structured fields for label + badge rendering", () => {
    const [row] = toUsageRows([usageRecord({ audio_seconds: 42 })]);
    expect(row.capability).toBe("translate");
    expect(row.origin).toBe("floating_menu");
    expect(row.terminalStatus).toBe("finished");
    expect(row.tokensIn).toBe(12);
    expect(row.tokensOut).toBe(8);
    expect(row.audioSeconds).toBe(42);
  });

  /**
   * 一条失败必须带得动「哪一档 + 什么码」。
   *
   * 真机 2026-08-31：弹窗翻译连续失败 9 次，账本里 9 行全是
   * `provider_id = "unresolved"` + `failed`，答不出任何一个有用的问题；而同一台服务端
   * 的资料蒸馏（批量档）同时全部成功——两组记录在这张表上此前看起来是同一件事。
   */
  it("失败行带着档位与错误码，且两者都进检索面", () => {
    const [row] = toUsageRows([
      usageRecord({
        terminal_status: "failed",
        service_class: "realtime",
        error_code: "SERVER_SIDE_FAILURE",
        provider_id: "unresolved",
        model_id: null,
      }),
    ]);
    expect(row.serviceClass).toBe("realtime");
    expect(row.errorCode).toBe("SERVER_SIDE_FAILURE");
    expect(row.searchText).toContain("realtime");
    expect(row.searchText).toContain("SERVER_SIDE_FAILURE");
  });

  it("补列之前的历史行没有档位、错误码与执行位置，检索面上也不该凭空多出空串", () => {
    const [row] = toUsageRows([
      usageRecord({ service_class: null, error_code: null, provider_location: null }),
    ]);
    expect(row.serviceClass).toBeNull();
    expect(row.errorCode).toBeNull();
    // 「在哪跑的」同理：补列之前的行没有它，不猜一个位置——猜错会让当时在服务端跑的
    // 任务在「本机省下」里被算成省下的钱（ADR-126 第 6 条）。
    expect(row.location).toBeNull();
    expect(row.searchText).not.toContain("  ");
  });

  it("执行位置进检索面：「这段时间哪些是在本机跑的」只有它答得了", () => {
    const [row] = toUsageRows([usageRecord({ provider_location: "cloud" })]);
    expect(row.location).toBe("cloud");
    expect(row.searchText).toContain("cloud");
  });

  it("没有 token 概念的任务保留 `null`，不折成 0", () => {
    // 本地转写、本地嵌入的 token 恒为 `null`。折成 0 之后，界面上"输入 0 / 输出 0"与
    // "这条根本没有 token 这回事"读起来一模一样——而这个面板唯一要避免的就是这个误读。
    // 空值一路留到渲染处：两端皆空时整条度量不出现，只缺一端时说「—」。
    const [row] = toUsageRows([
      usageRecord({ tokens_in: null, tokens_out: null, audio_seconds: null, model_id: null }),
    ]);
    expect(row.tokensIn).toBeNull();
    expect(row.tokensOut).toBeNull();
    // 音频时长不同：0 秒与"没有音频"在这个面板上是同一件事，且已有 `> 0` 的条件渲染。
    expect(row.audioSeconds).toBe(0);
    expect(row.model).toBe("default");
  });
});

describe("terminalStatusKind", () => {
  it("maps terminal status strings to TaskStatus kinds", () => {
    expect(terminalStatusKind("finished")).toBe("completed");
    expect(terminalStatusKind("succeeded")).toBe("completed");
    expect(terminalStatusKind("failed")).toBe("failed");
    expect(terminalStatusKind("error")).toBe("failed");
    expect(terminalStatusKind("degraded")).toBe("degraded");
    expect(terminalStatusKind("weird")).toBe("cancelled");
  });
});
