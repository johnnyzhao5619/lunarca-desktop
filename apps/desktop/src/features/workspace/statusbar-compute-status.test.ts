import { describe, expect, it } from "vitest";
import type { ComputeStatusDto } from "../../ipc/bindings";
import { projectStatusBarCompute } from "./statusbar-compute-status";

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
    ],
    local_candidates: [
      {
        capability: "chat",
        policy: "unreachable_before_visible_output",
        model_name: "Qwen Local",
        readiness: "ready",
        is_minimal: false,
        setting_key: "ai.assignment.default_llm",
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

describe("projectStatusBarCompute", () => {
  it("服务端托管不读取本机模型目录，零本机模型仍如实显示服务端路径", () => {
    expect(
      projectStatusBarCompute(
        status({
          location: "organization",
          chat_route_kind: "managed_service",
          routes: [{ ...status().routes[0], location: "server", reason: "server_default" }],
          local_candidates: [],
        }),
      ),
    ).toEqual({ kind: "managed_service", modelName: null });
  });

  it("本机就绪和本机不可用原因互不伪装", () => {
    expect(projectStatusBarCompute(status())).toEqual({
      kind: "device_ready",
      modelName: "Qwen Local",
    });
    expect(
      projectStatusBarCompute(
        status({
          local_candidates: [{ ...status().local_candidates[0], model_name: null, readiness: "model_missing" }],
        }),
      ),
    ).toEqual({ kind: "device_model_missing", modelName: null });
    expect(
      projectStatusBarCompute(
        status({
          local_candidates: [
            { ...status().local_candidates[0], readiness: "runtime_unavailable" },
          ],
        }),
      ),
    ).toEqual({ kind: "device_runtime_unavailable", modelName: "Qwen Local" });
  });

  it("共享快照未到时显示正在读取", () => {
    expect(projectStatusBarCompute(null)).toEqual({ kind: "loading", modelName: null });
  });
});
