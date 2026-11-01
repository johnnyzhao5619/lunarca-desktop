/**
 * 工作区观测的前端上报口（PRD-07-070 / TDD-12 §2.1）。
 *
 * **只在唯一的导航/动作宿主发送**（§2.1 明写）：两个展示面各报一次，同一次到访会被
 * 记成两次——而这条指标的全部用途是"用户到底去不去那一格"，翻倍之后它什么都不说明。
 *
 * **前端不判断能不能记**：授权判定在 Rust（关着时命令静默 no-op）。让前端先问一次
 * "能不能记"再决定要不要调，等于每个观测点都多一次往返，而它们本来就该是最便宜的一行。
 */

import { useEffect, useRef } from "react";
import {
  commands,
  type CreateActionIdDto,
  type WorkspaceMetricEntryResultDto,
  type WorkspaceMetricEntrySurfaceDto,
  type WorkspaceMetricSurfaceDto,
} from "../../ipc/bindings";
import { destinationKeyOf, type DestinationKey, type WorkspaceLocation } from "./workspace-ia";

/**
 * 目的地键 → 观测维度。
 *
 * **必须是 `Record<DestinationKey, …>` 而不是字符串变换**：第一版写的是
 * `sidebarKeyOf(location).replace("/", "_") as WorkspaceMetricSurfaceDto`——那个 `as` 把
 * 类型检查整个关掉了。多一格目的地时它照常编译，运行时送出一个 Rust 不认识的字符串，
 * 命令反序列化失败被静默丢弃：**库里从此少一个维度，而没有任何东西会红**。
 * 这正是那一版注释声称已经避免的事。
 *
 * 现在少一格当场编译不过。两边的取值恰好同形（`/` ↔ `_`）不能作为省掉这张表的理由——
 * "恰好同形"不是契约，它只是此刻为真。
 */
export const SURFACE_OF: Record<DestinationKey, WorkspaceMetricSurfaceDto> = {
  search: "search",
  notes: "notes",
  "knowledge/topics": "knowledge_topics",
  "knowledge/tags": "knowledge_tags",
  "knowledge/network": "knowledge_network",
  quick_notes: "quick_notes",
  recordings: "recordings",
  transcripts: "transcripts",
  chat: "chat",
  creator: "creator",
  apps: "apps",
  trash: "trash",
  settings: "settings",
};

/** 到访一格就记一次（同一格连续停留不重复记）。 */
export function useSurfaceMetrics(location: WorkspaceLocation): void {
  const lastRef = useRef<string>("");
  useEffect(() => {
    const surface = SURFACE_OF[destinationKeyOf(location)];
    if (surface === lastRef.current) {
      return;
    }
    lastRef.current = surface;
    void commands.recordWorkspaceMetric({ kind: "surface_viewed", surface });
  }, [location]);
}

/**
 * 记一次创建尝试的终态。
 *
 * `duration_ms` 由调用方从**用户发起那一刻**起算——Rust 只保留它落进哪个桶，原始毫秒
 * 不出这一次调用。
 */
export function recordCreateEntry(
  action: CreateActionIdDto,
  entry: WorkspaceMetricEntrySurfaceDto,
  result: WorkspaceMetricEntryResultDto,
  startedAt: number,
): void {
  void commands.recordWorkspaceMetric({
    kind: "create_entry_finished",
    action,
    entry,
    result,
    duration_ms: Math.max(0, Math.round(performance.now() - startedAt)),
  });
}
