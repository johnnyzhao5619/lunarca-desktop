// 语音 AI Mode 系统（F-GAP-02）。
// 前端只保存/展示稳定 mode id；Refine 指令由 Rust 在 voice_input 主干内生成（ADR-003）。

import type { VoiceRefineMode } from "../../ipc/bindings";
import { useCallback, useState } from "react";

export interface VoiceMode {
  id: VoiceRefineMode;
  /** i18n key 或 label（built-in mode 用 i18n key，用户自定义直接用 label） */
  label: string;
}

// ─── 内置 Mode 列表 ────────────────────────────────────────────────────────────

export const BUILTIN_VOICE_MODES: VoiceMode[] = [
  {
    id: "general",
    label: "voice-input:mode.general",
  },
  {
    id: "email",
    label: "voice-input:mode.email",
  },
  {
    id: "slack",
    label: "voice-input:mode.slack",
  },
  {
    id: "code_comment",
    label: "voice-input:mode.codeComment",
  },
  {
    id: "raw",
    label: "voice-input:mode.raw",
  },
];

const ACTIVE_MODE_KEY = "luna:voice-mode-active";

// ─── Storage helpers ────────────────────────────────────────────────────────

export function loadActiveModeId(): VoiceRefineMode {
  const stored = localStorage.getItem(ACTIVE_MODE_KEY);
  return BUILTIN_VOICE_MODES.some((mode) => mode.id === stored)
    ? (stored as VoiceRefineMode)
    : "general";
}

export function saveActiveModeId(id: VoiceRefineMode): void {
  localStorage.setItem(ACTIVE_MODE_KEY, id);
}

/** 供 VoiceInputHud 使用的轻量 hook */
export function usePersistentVoiceMode() {
  const [activeModeId, setActiveModeIdState] = useState<VoiceRefineMode>(() => loadActiveModeId());
  const allModes = BUILTIN_VOICE_MODES;
  const activeMode = allModes.find((m) => m.id === activeModeId) ?? BUILTIN_VOICE_MODES[0];

  const setActiveModeId = useCallback((id: VoiceRefineMode) => {
    saveActiveModeId(id);
    setActiveModeIdState(id);
  }, []);

  return {
    allModes,
    activeMode,
    activeModeId,
    setActiveModeId,
  };
}
