/**
 * 说话人呈现层纯逻辑（PRD-04-056 / PRD-04-054）：显示名解析与配色槽位。
 *
 * 无 IO、无 React，可 headless 测试。业务（归属、拆分、合并、新 id 派生）全在 Rust
 * （ADR-003 前端零业务）——此处只负责「同一个 speaker id 在任何表面都显示同一个名字、
 * 同一种颜色」。
 */

import type { SpeakerStatDto } from "../../ipc/bindings";

/** 配色槽位数（`--speaker-1..6`，超出即循环复用；6 位以上会议已远超人眼可辨色阈值）。 */
export const SPEAKER_SLOT_COUNT = 6;

/** 默认轨标签文案（由调用方从 i18n 注入，保持本模块与 i18n 解耦、可测）。 */
export interface SpeakerLabelStrings {
  /** `self` = 我。 */
  self: string;
  /** `remote` = 对方（未细分的粗轨标签）。 */
  remote: string;
  /** `remote:N` = 对方 N。 */
  remoteN: (n: string) => string;
  /** `speaker:N` = 说话人 N（现场麦克风细分）。 */
  speakerN: (n: string) => string;
}

/**
 * speaker id → 显示名。优先级：笔记 frontmatter 改名映射 → 默认轨标签 → 原 id 兜底
 * （与后端 `speaker_display_name` 同一优先级链，导出/纪要因此与界面一致）。
 */
export function speakerDisplayName(
  id: string,
  names: Record<string, string>,
  labels: SpeakerLabelStrings,
): string {
  const custom = names[id]?.trim();
  if (custom) return custom;
  if (id === "self") return labels.self;
  if (id === "remote") return labels.remote;
  const remoteN = /^remote:(\d+)$/.exec(id);
  if (remoteN) return labels.remoteN(remoteN[1]);
  const speakerN = /^speaker:(\d+)$/.exec(id);
  if (speakerN) return labels.speakerN(speakerN[1]);
  return id;
}

/**
 * speaker id → 配色槽位（1..{@link SPEAKER_SLOT_COUNT}；0 = 不在名册，走中性配色）。
 *
 * 槽位取名册下标——名册按**首次出现时间**排序（后端 `speaker_roster`），因此同一录音里
 * 颜色稳定：改名、重指派都不会让既有说话人换色，只有名册增删才平移新成员。
 */
export function speakerSlot(id: string | null, roster: readonly SpeakerStatDto[]): number {
  if (!id) return 0;
  const index = roster.findIndex((stat) => stat.id === id);
  if (index < 0) return 0;
  return (index % SPEAKER_SLOT_COUNT) + 1;
}
