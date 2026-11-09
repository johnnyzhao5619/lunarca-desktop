/**
 * 转写槽位「为什么选不到服务端托管」守护（真机 2026-11-03）。
 *
 * 同意开关在上方算力卡，下拉里一句不提时，用户读到的是「选择器坏了」。拦两件事：
 * 下拉里有一个灰掉、看得见的「服务端托管」；那一句原因与去改的入口真的渲染出来，
 * 且文案键都在资源包里（缺键时 i18next 回落成裸键，与译文不等即红）。
 */

import { createElement } from "react";
import { expect, it } from "vitest";

import type { AsrServerGate, ModelAssignmentDto } from "../../ipc/bindings";
import { SlotRow } from "./ModelManagementPanel";
import { renderWithSettingsHosts, translateForTest } from "./settings-test-fixture";

function asrSlot(server_gate: AsrServerGate | null, serverCandidate = false): ModelAssignmentDto {
  return {
    slot: "realtime_asr",
    setting_key: "ai.assignment.realtime_asr",
    source: "device_policy",
    readiness: "ready",
    configured_target: null,
    effective_target: "whisper-tiny-bundled",
    effective_name: "Whisper Tiny (q5_1)",
    required_capability: "transcribe",
    selection_mode: "model",
    follow_is_server_managed: false,
    server_gate,
    candidates: [
      { target_id: "whisper-tiny-bundled", name: "Whisper Tiny (q5_1)", location: "device" },
      ...(serverCandidate ? [{ target_id: "service:asr", name: "", location: "server" as const }] : []),
    ],
  };
}

const render = (assignment: ModelAssignmentDto) =>
  renderWithSettingsHosts(
    createElement(SlotRow, {
      assignment,
      assign: () => Promise.resolve(true),
      clearAssignment: () => Promise.resolve(true),
    }),
  );

it("还没同意：下拉里有灰掉的服务端项，下面说差的是同意并带去开关", () => {
  const html = render(asrSlot({ kind: "consent_off" }));
  expect(html).toContain(translateForTest("settings:models.managed.gate.consent_option"));
  expect(html).toMatch(/<option[^>]*disabled[^>]*>[^<]*服务端托管/u);
  expect(html).toContain(translateForTest("settings:models.managed.gate.consent_off"));
  expect(html).toContain(translateForTest("settings:models.managed.gate.open_consent"));
});

it("同意项勾不动：照搬算力卡的原因，不叫人去勾一个勾不动的开关", () => {
  const html = render(asrSlot({ kind: "blocked", blocker: "local_only_strategy" }));
  expect(html).toContain(translateForTest("settings:models.managed.gate.blocked_option"));
  expect(html).toContain(translateForTest("settings:compute_card.transcription_local_only"));
  expect(html).toContain(translateForTest("settings:models.managed.gate.change_strategy"));
  expect(html).not.toContain(translateForTest("settings:models.managed.gate.open_consent"));
});

it("选得到服务端时不再摆灰项；没有闸门就什么都不说", () => {
  const open = render(asrSlot({ kind: "blocked", blocker: "server_not_configured" }, true));
  expect(open).not.toContain(translateForTest("settings:models.managed.gate.blocked_option"));
  const none = render(asrSlot(null));
  expect(none).not.toContain(translateForTest("settings:models.managed.gate.consent_off"));
  expect(none).not.toMatch(/<option[^>]*disabled/u);
});
