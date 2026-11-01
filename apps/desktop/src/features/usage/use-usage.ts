import { useEffect, useRef, useState } from "react";
import { commands, type UsageRecordDto, type UsageSummaryDto } from "../../ipc/bindings";

/**
 * 结构化用量行：供「用量与执行记录」渲染人话标签/状态徽标/格式化用量（不再拼裸串）。
 *
 * **本模块随职责搬到 `features/usage/`（ADR-087 ②）**：它读的是 `list_usage` /
 * `usage_summary`，答的是「一段时间做了什么、花了多少」——那是审计投影，与「有什么值得
 * 知道」的通知历史同源不同任务。此前它叫 `activity/use-activity.ts`，名字跟着那个把两件事
 * 塞进一页的「动态」容器走；容器退役后名字必须跟着**真实职责**走，否则下一个人会按
 * 「活动」去找它，然后在别处再写一份。
 */
export interface UsageRow {
  id: string;
  /** 稳定 snake_case 能力名（i18n 标签 key）。 */
  capability: string;
  /** 稳定 snake_case 来源名（i18n 标签 key）。 */
  origin: string;
  /** 终态字符串（i18n 标签 key，如 finished/failed/cancelled）。 */
  terminalStatus: string;
  providerId: string;
  /**
   * 这一次在哪跑的（`device` / `local_service` / `cloud`）；`null` = 补列之前的历史行。
   *
   * 面板导语一直写着「在哪跑的」，而此前行里只有 provider id 与模型名两个机器串
   * （ADR-126 第 6 条）。判定在 Rust，前端不从 id 前缀猜。
   */
  location: string | null;
  model: string;
  /**
   * 这一次打的是哪一档服务（`realtime` / `batch`）；`null` = 补列之前写下的历史行。
   *
   * 与 `capability` 一起才说得清「打了哪条链」：服务端按 `(能力, 档位)` 取链，
   * 同一个能力的两档可以一条好一条坏（真机 2026-08-31 就是这个形状）。
   */
  serviceClass: string | null;
  /** 失败终态的稳定错误码；成功与取消为 `null`。渲染处翻成人话，**绝不裸显**。 */
  errorCode: string | null;
  /** `null` = 这条任务没有 token 概念（本地转写、本地嵌入），不是"用了 0 个"。 */
  tokensIn: number | null;
  tokensOut: number | null;
  audioSeconds: number;
  searchText: string;
  finishedAtMs: number;
}

function parseMs(value: string): number {
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) ? parsed : 0;
}

export function toUsageRows(records: UsageRecordDto[]): UsageRow[] {
  return records.map((record) => {
    const model = record.model_id ?? "default";
    return {
      id: record.job_id,
      capability: record.capability,
      origin: record.origin,
      terminalStatus: record.terminal_status,
      providerId: record.provider_id,
      location: record.provider_location,
      model,
      serviceClass: record.service_class,
      errorCode: record.error_code,
      // 不折成 0：界面上"0"与"这条没有 token 概念"读起来一模一样，而这个面板唯一要
      // 避免的误读就是这个。空值一路留到渲染处再决定怎么说。
      tokensIn: record.tokens_in,
      tokensOut: record.tokens_out,
      audioSeconds: record.audio_seconds ?? 0,
      // 档位与错误码进检索面：「上个月那批实时档失败」这种问题只有它们答得了，
      // 而这个面板存在的理由就是回答这类问题。
      searchText: [
        record.capability,
        record.origin,
        record.provider_id,
        record.provider_location,
        model,
        record.terminal_status,
        record.service_class,
        record.error_code,
      ]
        .filter(Boolean)
        .join(" "),
      finishedAtMs: parseMs(record.finished_at_ms),
    };
  });
}

/** 终态字符串 → TaskStatus tone（着色用；finished=完成，未知归 muted）。 */
export function terminalStatusKind(status: string): "completed" | "failed" | "cancelled" | "degraded" {
  switch (status) {
    case "finished":
    case "completed":
    case "succeeded":
      return "completed";
    case "failed":
    case "error":
      return "failed";
    case "degraded":
      return "degraded";
    default:
      return "cancelled";
  }
}

/**
 * 「这本账变了，重读一次」——**写账的那一方与显示账的那一方不在同一棵子树里**。
 *
 * 真机 2026-09-12：「试跑一次」跑完说「这一次已记入下方『执行记录』」，而下方那张表
 * 要手动点一次「刷新」才看得到它——文案承诺了界面没兑现。账是 Rust 在 Job 终态时写的，
 * 前端这边只有取数那一刻的快照，没有人通知它。
 *
 * 收在模块级而不是让调用方互相传回调：发起 AI 动作的地方遍布全应用（自检只是第一个
 * 撞上的），它们没有理由认识执行记录那张表（同 `use-compute-status` 的共享订阅形态）。
 */
const listeners = new Set<() => void>();

/** 刚写过一条用量记录的地方调它；没有人在看这张表时它什么都不做。 */
export function refreshUsage(): void {
  for (const listener of listeners) listener();
}

export function useUsage() {
  const [records, setRecords] = useState<UsageRecordDto[]>([]);
  const [summary, setSummary] = useState<UsageSummaryDto | null>(null);
  const [loading, setLoading] = useState(true);

  async function refresh() {
    setLoading(true);
    const [usageRecords, usageSummary] = await Promise.all([
      commands.listUsage(200),
      commands.usageSummary(),
    ]);
    setRecords(usageRecords);
    setSummary(usageSummary);
    setLoading(false);
  }

  // 注册的是一个稳定包装：`refresh` 每次渲染都是新闭包，直接塞进集合会越积越多，
  // 而它们各自捕获着一份过期的 setState 链。
  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;
  useEffect(() => {
    const notify = () => void refreshRef.current();
    listeners.add(notify);
    return () => {
      listeners.delete(notify);
    };
  }, []);

  async function clear() {
    await commands.clearUsage();
    await refresh();
  }

  useEffect(() => {
    void refresh();
  }, []);

  return {
    rows: toUsageRows(records),
    summary,
    loading,
    refresh,
    clear,
  };
}
