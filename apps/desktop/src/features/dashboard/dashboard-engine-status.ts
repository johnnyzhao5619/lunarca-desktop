/**
 * 启动概览的 AI 算力投影。
 *
 * 这里只把 Rust `compute_status` 已裁决的执行路径翻译成有限 UI 状态；不从本机模型目录
 * 反推服务端能力。模型目录只用于回答「明确在本机执行的 LLM 具体是哪一个」。
 */

import type { ComputeStatusDto, ModelEntryDto } from "../../ipc/bindings";

export type DashboardComputePulse =
  | "loading"
  | "unavailable"
  | "needs_setup"
  | "server"
  | "device";

export type DashboardEngineSource =
  | "managed_service"
  | "device"
  | "unavailable";

export interface DashboardEngineItem {
  source: DashboardEngineSource;
  modelName: string | null;
  ready: boolean;
}

export interface DashboardEngineProjection {
  pulse: DashboardComputePulse;
  asr: DashboardEngineItem;
  llm: DashboardEngineItem;
}

function unavailableItem(): DashboardEngineItem {
  return { source: "unavailable", modelName: null, ready: false };
}

export function projectDashboardEngines(
  status: ComputeStatusDto | null,
  models: readonly ModelEntryDto[],
  resolved: boolean,
): DashboardEngineProjection {
  if (!resolved) {
    return { pulse: "loading", asr: unavailableItem(), llm: unavailableItem() };
  }
  if (!status) {
    return { pulse: "unavailable", asr: unavailableItem(), llm: unavailableItem() };
  }

  const transcriptionRoute = status.routes.find((route) => route.capability === "transcription");
  const chatRoute = status.routes.find((route) => route.capability === "chat");
  const localTranscription = status.local_candidates.find(
    (candidate) => candidate.capability === "transcription",
  );

  const asr: DashboardEngineItem =
    transcriptionRoute?.location === "server"
      ? { source: "managed_service", modelName: null, ready: true }
      : transcriptionRoute?.location === "device"
        ? {
            source: "device",
            modelName: localTranscription?.model_name ?? null,
            ready: localTranscription?.readiness === "ready",
          }
        : unavailableItem();

  const localLlm = models.find((model) => model.assigned_slots.includes("default_llm"));
  const llm: DashboardEngineItem =
    chatRoute?.location === "server"
      ? { source: "managed_service", modelName: null, ready: true }
      : chatRoute?.location === "device"
        ? { source: "device", modelName: localLlm?.name ?? null, ready: Boolean(localLlm) }
        : unavailableItem();

  const pulse: DashboardComputePulse =
    !asr.ready || !llm.ready
      ? "needs_setup"
      : asr.source === "managed_service" || llm.source === "managed_service"
        ? "server"
        : "device";

  return { pulse, asr, llm };
}
