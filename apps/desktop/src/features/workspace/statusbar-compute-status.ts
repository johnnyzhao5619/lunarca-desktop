/**
 * 工作区底栏的文本 AI 执行路径投影。
 *
 * 只消费 Rust `compute_status` 已裁决的 Chat 路由及本机候选就绪结论；不得从模型目录、
 * 端点地址或模型名反推服务端是否可用（PRD-07-052 / ADR-003）。
 */

import type { ComputeStatusDto, LocalCandidateReadiness } from "../../ipc/bindings";

export type StatusBarComputeKind =
  | "loading"
  | "unavailable"
  | "managed_service"
  | "device_ready"
  | "device_model_missing"
  | "device_runtime_unavailable";

export interface StatusBarComputeProjection {
  kind: StatusBarComputeKind;
  modelName: string | null;
}

const UNAVAILABLE: StatusBarComputeProjection = { kind: "unavailable", modelName: null };

function deviceKind(readiness: LocalCandidateReadiness | undefined): StatusBarComputeKind {
  switch (readiness) {
    case "ready":
      return "device_ready";
    case "runtime_unavailable":
      return "device_runtime_unavailable";
    case "model_missing":
    case "not_applicable":
    case undefined:
      return "device_model_missing";
  }
}

/** `status = null` = 共享快照还没到（`useComputeStatus`），说「正在读取」而不是「不可用」。 */
export function projectStatusBarCompute(status: ComputeStatusDto | null): StatusBarComputeProjection {
  if (!status) return { kind: "loading", modelName: null };

  const chatRoute = status.routes.find((route) => route.capability === "chat");
  if (!chatRoute) return UNAVAILABLE;
  if (chatRoute.location === "server") return { kind: "managed_service", modelName: null };
  if (chatRoute.location !== "device") return UNAVAILABLE;

  const localChat = status.local_candidates.find((candidate) => candidate.capability === "chat");
  return {
    kind: deviceKind(localChat?.readiness),
    modelName: localChat?.model_name ?? null,
  };
}
