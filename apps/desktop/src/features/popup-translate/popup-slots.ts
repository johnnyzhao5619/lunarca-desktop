import type { TranslateMeta } from "../../ipc/bindings";

export const POPUP_MAX_SLOTS = 2;

export type PopupSlotStatus =
  | "idle"
  | "running"
  | "done"
  | "cache_hit"
  | "failed"
  | "cancelled"
  | "error";

export type PopupSlot = {
  id: number;
  presetId: string;
  output: string;
  meta: TranslateMeta | null;
  status: PopupSlotStatus;
  message: string | null;
  /** 失败时的稳定错误码（映射修复动作按钮，如 MODEL_MISSING → 打开模型设置）。 */
  failureCode: string | null;
  savedPath: string | null;
};

export function createPopupSlot(id: number, presetId: string): PopupSlot {
  return {
    id,
    presetId,
    output: "",
    meta: null,
    status: "idle",
    message: null,
    failureCode: null,
    savedPath: null,
  };
}

export function defaultPresetId(presetIds: string[]) {
  return presetIds.includes("general") ? "general" : presetIds[0] ?? "";
}

export function createInitialSlots(presetIds: string[]) {
  const presetId = defaultPresetId(presetIds);
  return presetId ? [createPopupSlot(1, presetId)] : [];
}

export function nextComparePresetId(presetIds: string[], slots: PopupSlot[]) {
  const used = new Set(slots.map((slot) => slot.presetId));
  return presetIds.find((presetId) => !used.has(presetId)) ?? presetIds[0] ?? "";
}

export function canAddCompareSlot(slots: PopupSlot[], presetCount: number) {
  return slots.length < POPUP_MAX_SLOTS && presetCount > 0;
}

/** 移除对比槽（主槽 id=1 不可移除）。 */
export function removeCompareSlot(slots: PopupSlot[], slotId: number) {
  if (slotId === 1) return slots;
  return slots.filter((slot) => slot.id !== slotId);
}

/**
 * 结果区是否存在（ADR-112 ④：首 token 才出现）——任一槽离开 idle 即算。
 * 展示模式一进来就自动生成，故它天然为真；输入态在生成前恒为假。
 */
export function resultsVisible(slots: PopupSlot[]) {
  return slots.some((slot) => slot.status !== "idle");
}

export function setSlotRunning(slot: PopupSlot, presetId = slot.presetId): PopupSlot {
  return {
    ...slot,
    presetId,
    output: "",
    meta: null,
    status: "running",
    message: null,
    savedPath: null,
  };
}

export function applySlotMeta(slot: PopupSlot, meta: TranslateMeta): PopupSlot {
  return { ...slot, meta };
}

export function applySlotToken(slot: PopupSlot, token: string): PopupSlot {
  return { ...slot, output: slot.output + token };
}

export function applySlotDone(slot: PopupSlot, output: string): PopupSlot {
  return { ...slot, output, status: "done", message: null, failureCode: null };
}

export function applySlotCacheHit(slot: PopupSlot, output: string): PopupSlot {
  return { ...slot, output, status: "cache_hit", message: null, failureCode: null };
}

export function applySlotTerminal(
  slot: PopupSlot,
  status: Extract<PopupSlotStatus, "failed" | "cancelled" | "error">,
  message: string | null,
): PopupSlot {
  return { ...slot, status, message, failureCode: null };
}

/** 生成失败终态：本地化文案 + 稳定错误码（驱动修复动作按钮）。 */
export function applySlotFailure(
  slot: PopupSlot,
  code: string,
  message: string,
): PopupSlot {
  return { ...slot, status: "failed", message, failureCode: code };
}
