import { type PresetInfo, type TranslateMeta } from "../../ipc/bindings";
import { langLabelKey } from "../settings/language-options";

type Translate = (key: string, options?: Record<string, unknown>) => string;

/**
 * 预设展示名：`popup:preset.<id>` 有译文就用译文，没有才回落注册表里的名字。
 * 下拉与摘要行**共用这一份**（M1：此前摘要行直出 Rust 发来的中文名，与紧挨着的下拉
 * 「General」同屏两种语言）。
 */
export function presetLabel(preset: PresetInfo, t: Translate): string {
  const key = `preset.${preset.id}`;
  const translated = t(key);
  return translated === key ? preset.name : translated;
}

/**
 * 结果摘要行「翻译 → 中文 · 通用翻译」（PRD-01-029）。
 *
 * - 目标语走 `common:lang.<code>` 的语言显示名（与字幕语言流条同一来源），集外语码回落裸码；
 * - 预设按 id 经 [`presetLabel`] 解析；快照里找不到的 id（注册表已换）回落成 id 本身，
 *   而不是让整行消失。
 */
export function metaLabel(
  meta: TranslateMeta,
  presets: PresetInfo[],
  t: Translate,
  tCommon: Translate,
): string {
  const mode = meta.mode === "refine" ? t("meta.refine") : t("meta.translate");
  const target = tCommon(langLabelKey(meta.target).slice("common:".length), {
    defaultValue: meta.target,
  });
  const preset = presets.find((candidate) => candidate.id === meta.preset_id);
  const presetName = preset ? presetLabel(preset, t) : meta.preset_id;
  return meta.uncertain
    ? t("meta.summaryUncertain", {
        mode,
        target,
        preset: presetName,
        uncertain: t("meta.uncertain"),
      })
    : t("meta.summary", { mode, target, preset: presetName });
}
