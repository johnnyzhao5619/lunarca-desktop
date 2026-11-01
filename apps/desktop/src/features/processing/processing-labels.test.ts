import { describe, expect, it } from "vitest";
import type { ProcStatus, ProcTaskDto } from "../../ipc/bindings";
import {
  isTaskActive,
  isTaskDone,
  isTaskRetryable,
  isTaskUnsuccessful,
  procKindLabel,
  procProgressCount,
  procRunningLabel,
  procServerQueueLabel,
} from "./processing-labels";

function task(status: ProcStatus, queue: ProcTaskDto["queue"] = null): ProcTaskDto {
  // `executed_by: null` = 本机跑的（绝大多数用例的情形）；
  // `queue: null` = 不在服务端队列里（本机跑的，或服务端已经开始解码）。
  return {
    id: "1",
    kind: "import",
    label: "a.mp3",
    note_rel: null,
    status,
    executed_by: null,
    queue,
    restored: false,
  };
}

/** i18n 替身：回「key + 插值」，用例据此断言选了哪条文案、带了哪些数。 */
const t = (key: string, options?: Record<string, unknown>) =>
  options === undefined ? key : `${key}(${JSON.stringify(options)})`;

const running: ProcStatus = {
  state: "running",
  data: { step: "awaiting_server", done: 0, total: 0 },
};

describe("服务端排位文案（ADR-049 ⑧）", () => {
  it("不在服务端队列里就什么都不显示", () => {
    expect(procServerQueueLabel(task({ state: "queued" }), t)).toBeNull();
  });

  it("服务端还没有样本时**只报位次**，绝不补一个估出来的时间", () => {
    // 刚部署完的服务端正是这种状态，而那也正是用户最会去看等待预估的时刻。
    // 编一个数会被当成承诺，而它没有任何依据。
    const label = procServerQueueLabel(
      task(running, { position: 3, tier: "batch", eta_min_secs: null, eta_max_secs: null }),
      t,
    );
    expect(label).toContain("processing.serverQueue.position");
    expect(label).not.toContain("eta");
  });

  it("有区间就给区间，且下界不会大于上界", () => {
    const label = procServerQueueLabel(
      task(running, { position: 1, tier: "standard", eta_min_secs: 120, eta_max_secs: 400 }),
      t,
    );
    expect(label).toContain("processing.serverQueue.eta");
    expect(label).toContain('"min":2');
    expect(label).toContain('"max":7');
  });

  it("一分钟以内说「不到 1 分钟」，不说「0–1 分钟」", () => {
    // 「0–1 分钟」读起来像「可能马上、也可能永远」，而真实含义是「快好了」。
    const label = procServerQueueLabel(
      task(running, { position: 1, tier: "standard", eta_min_secs: 5, eta_max_secs: 40 }),
      t,
    );
    expect(label).toContain("processing.serverQueue.soon");
  });

  it("上下界四舍五入到同一分钟时收敛成一个数", () => {
    const label = procServerQueueLabel(
      task(running, { position: 2, tier: "batch", eta_min_secs: 115, eta_max_secs: 125 }),
      t,
    );
    expect(label).toContain("processing.serverQueue.etaSingle");
    expect(label).toContain('"minutes":2');
  });
});

describe("处理队列任务状态谓词", () => {
  it("排队与运行中算活跃", () => {
    expect(isTaskActive(task({ state: "queued" }))).toBe(true);
    expect(
      isTaskActive(task({ state: "running", data: { step: "transcribing", done: 1, total: 3 } })),
    ).toBe(true);
  });

  it("三种终态一律不算活跃（角标/spinner/互斥按钮据此复位）", () => {
    // PRD-05-010 引入 cancelled 时，顶栏角标与回放内联互斥都曾把它误判为进行中，
    // 导致取消后角标常亮、按钮永久禁用。谓词收敛为单一来源后由本测试锁定。
    expect(isTaskActive(task({ state: "done", data: { note_rel: "T/a.md", awaiting_review: 0 } }))).toBe(false);
    expect(
      isTaskActive(task({ state: "failed", data: { code: "DECODE_FAILED", retryable: true, retry_charges_again: false } })),
    ).toBe(false);
    expect(isTaskActive(task({ state: "cancelled" }))).toBe(false);
  });
});

describe("三段谓词互斥且穷尽（PRD-04-055 三段修订）", () => {
  it("每个状态恰好落在进行中 / 未成功 / 已完成之一", () => {
    const statuses: ProcStatus[] = [
      { state: "prepared" },
      { state: "queued" },
      running,
      { state: "done", data: { note_rel: null, awaiting_review: 0 } },
      { state: "failed", data: { code: "X", retryable: true, retry_charges_again: false } },
      { state: "cancelled" },
    ];
    for (const status of statuses) {
      const hits = [isTaskActive, isTaskUnsuccessful, isTaskDone].filter((p) => p(task(status)));
      expect(hits, status.state).toHaveLength(1);
    }
  });

  it("失败与取消都算未成功——它们是待重试的工作，不是历史", () => {
    expect(isTaskUnsuccessful(task({ state: "failed", data: { code: "X", retryable: false, retry_charges_again: false } }))).toBe(true);
    expect(isTaskUnsuccessful(task({ state: "cancelled" }))).toBe(true);
    expect(isTaskUnsuccessful(task({ state: "done", data: { note_rel: null, awaiting_review: 0 } }))).toBe(false);
  });
});

describe("处理队列展示标签", () => {
  const t = (key: string, options?: Record<string, unknown>) =>
    key === "processing.kindPhase"
      ? `${String(options?.kind)} · ${String(options?.phase)}`
      : key;

  it("非运行态只给类型标签", () => {
    expect(procKindLabel("import", t)).toBe("processing.kind.import");
    expect(procRunningLabel(task({ state: "queued" }), t)).toBe("processing.kind.import");
    expect(procRunningLabel(task({ state: "cancelled" }), t)).toBe("processing.kind.import");
  });

  it("运行态给「类型 · 阶段」", () => {
    const running = task({ state: "running", data: { step: "ingesting", done: 0, total: 0 } });
    expect(procRunningLabel(running, t)).toBe(
      "processing.kind.import · processing.phase.ingesting",
    );
  });
});

describe("进度读数按阶段分派单位（PRD-05-006）", () => {
  it("下载阶段的计数是 KiB，须格式化为可读体积", () => {
    // 后端以 KiB 上报（u32 装不下字节数）；裸显示会变成「12678 / 90234」这种没人看得懂的数。
    // 单位与取整规则来自共用的 `lib/format-bytes`（IEC 单位 + 三位有效数字）——不为下载
    // 另造一套体积格式，也不在这里把 1024 进制的数标成 `MB`。
    const fetching = task({
      state: "running",
      data: { step: "fetching", done: 12_678, total: 90_234 },
    });
    expect(procProgressCount(fetching, "en-US")).toBe("12.4\u00a0MiB / 88.1\u00a0MiB");
  });

  it("下载总量未知时只报已下载量，不编造分母", () => {
    const unknown = task({ state: "running", data: { step: "fetching", done: 2048, total: 0 } });
    expect(procProgressCount(unknown, "en-US")).toBe("2\u00a0MiB");
  });

  it("其余阶段仍是件数计数；不定量给 null（只转圈）", () => {
    const transcribing = task({
      state: "running",
      data: { step: "transcribing", done: 2, total: 5 },
    });
    expect(procProgressCount(transcribing, "en-US")).toBe("2 / 5");
    const translating = task({
      state: "running",
      data: { step: "translating", done: 0, total: 0 },
    });
    expect(procProgressCount(translating, "en-US")).toBeNull();
    expect(procProgressCount(task({ state: "queued" }), "en-US")).toBeNull();
  });
});

describe("重试按钮的可见性（PRD-05-006）", () => {
  it("失败项按 Rust 下发的 retryable 决定给不给重试", () => {
    // 环境类失败（网络/工具缺失）：装好或恢复后原样重跑就成。
    expect(
      isTaskRetryable(task({ state: "failed", data: { code: "FETCH_NETWORK", retryable: true, retry_charges_again: false } })),
    ).toBe(true);
    // 站点侧硬事实：重试只会重复失败，给按钮等于诱导用户空等。
    expect(
      isTaskRetryable(
        task({ state: "failed", data: { code: "FETCH_UNAVAILABLE", retryable: false, retry_charges_again: false } }),
      ),
    ).toBe(false);
  });

  it("上次会话回填的历史项不可重试——没有载荷，重跑无从谈起（PRD-04-062）", () => {
    const failed = task({ state: "failed", data: { code: "X", retryable: true, retry_charges_again: false } });
    expect(isTaskRetryable({ ...failed, restored: true })).toBe(false);
    expect(isTaskRetryable({ ...task({ state: "cancelled" }), restored: true })).toBe(false);
  });

  it("取消恒可重试；完成与进行中不给重试", () => {
    expect(isTaskRetryable(task({ state: "cancelled" }))).toBe(true);
    expect(isTaskRetryable(task({ state: "done", data: { note_rel: "T/a.md", awaiting_review: 0 } }))).toBe(false);
    expect(isTaskRetryable(task({ state: "queued" }))).toBe(false);
    expect(
      isTaskRetryable(task({ state: "running", data: { step: "fetching", done: 1, total: 2 } })),
    ).toBe(false);
  });
});
