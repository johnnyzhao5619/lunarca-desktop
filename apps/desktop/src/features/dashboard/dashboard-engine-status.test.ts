import { describe, expect, it } from "vitest";
import type { ComputeStatusDto, ModelEntryDto } from "../../ipc/bindings";
import { projectDashboardEngines } from "./dashboard-engine-status";

function status(overrides: Partial<ComputeStatusDto> = {}): ComputeStatusDto {
  return {
    location: "on_device",
    routes: [
      {
        capability: "chat",
        location: "device",
        reason: "device_policy",
        setting_key: "ai.assignment.default_llm",
        can_use_server: false,
      },
      {
        capability: "embedding",
        location: "device",
        reason: "device_policy",
        setting_key: "ai.assignment.default_embedding",
        can_use_server: false,
      },
      {
        capability: "transcription",
        location: "device",
        reason: "device_policy",
        setting_key: "ai.assignment.realtime_asr",
        can_use_server: false,
      },
    ],
    local_candidates: [
      {
        capability: "chat",
        policy: "unreachable_before_visible_output",
        model_name: "Qwen3 0.6B",
        readiness: "ready",
        is_minimal: false,
        setting_key: "ai.assignment.default_llm",
      },
      {
        capability: "embedding",
        policy: "explicit_only",
        model_name: null,
        readiness: "not_applicable",
        is_minimal: false,
        setting_key: "ai.assignment.default_embedding",
      },
      {
        capability: "transcription",
        policy: "unreachable_before_visible_output",
        model_name: "Whisper Small",
        readiness: "ready",
        is_minimal: false,
        setting_key: "ai.assignment.realtime_asr",
      },
    ],
    pending_handover: [],
    chat_route_kind: "on_device",
    transcription_server_blocker: "no_server",
    outbound_blocked_by_strategy: false,
    server_setup_incomplete: false,
    ...overrides,
  };
}

function model(name: string): ModelEntryDto {
  return {
    id: name.toLowerCase().replace(/ /g, "-"),
    name,
    declared_capabilities: ["chat"],
    effective_capabilities: ["chat"],
    origin: "remote",
    status: "installed",
    managed_by: "LunaManaged",
    license_label: "MIT",
    size_bytes: 1,
    est_on_disk_bytes: 1,
    on_disk_bytes: 1,
    runtime_digest: "sha256:test",
    quantization: null,
    estimated_memory_bytes: 1,
    device_fit: "supported",
    assigned_slots: ["default_llm"],
    partial_bytes: null,
    coreml_available: false,
    coreml_installed: false,
    coreml_size_bytes: null,
    coreml_partial_bytes: null,
    update_size_bytes: null,
  };
}

describe("projectDashboardEngines", () => {
  it("服务端托管时本机零模型也判为可用，不再恒显引擎未就绪", () => {
    const remote = status({
      location: "organization",
      chat_route_kind: "managed_service",
      routes: status().routes.map((route) => ({ ...route, location: "server" })),
    });
    const projected = projectDashboardEngines(remote, [], true);
    expect(projected.pulse).toBe("server");
    expect(projected.asr).toMatchObject({ source: "managed_service", ready: true });
    expect(projected.llm).toMatchObject({ source: "managed_service", ready: true });
  });

  it("明确走本机时才用本机模型分配判断是否需要设置", () => {
    expect(projectDashboardEngines(status(), [], true).pulse).toBe("needs_setup");
    expect(projectDashboardEngines(status(), [model("Qwen Local")], true)).toMatchObject({
      pulse: "device",
      llm: { source: "device", modelName: "Qwen Local", ready: true },
    });
  });

  it("加载中与命令失败分开表达", () => {
    expect(projectDashboardEngines(null, [], false).pulse).toBe("loading");
    expect(projectDashboardEngines(null, [], true).pulse).toBe("unavailable");
  });
});
