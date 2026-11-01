import { useTranslation } from "react-i18next";
import type { PresetInfo } from "../../ipc/bindings";
import { SelectBox } from "../../components/ui/SelectBox";
import { presetLabel } from "./popup-meta-label";

type PresetSelectProps = {
  presets: PresetInfo[];
  value: string;
  onChange: (presetId: string) => void;
  className?: string;
};

/**
 * 目标语言 / 表达方式的紧凑分组选择器（PRD-01-012/015）——窗口栏配置胶囊与每个结果槽共用。
 *
 * 分组由 Rust 下发的 `group` 决定（`target_lang` = 固定目标语言的预设 / `style` = 智能
 * 双向 + 风格片段），此前是前端按 `id.startsWith("to_")` 猜——那是前端在按字符串推业务
 * 分类（ADR-003）。
 */
export function PresetSelect({ presets, value, onChange, className }: PresetSelectProps) {
  const { t } = useTranslation("popup");
  const targetPresets = presets.filter((preset) => preset.group === "target_lang");
  const stylePresets = presets.filter((preset) => preset.group !== "target_lang");
  return (
    <SelectBox
      size="sm"
      className={className ? `luna-popup-slot__preset ${className}` : "luna-popup-slot__preset"}
      value={value}
      ariaLabel={t("presetSelector.label")}
      onChange={(event) => onChange(event.currentTarget.value)}
    >
      {targetPresets.length > 0 ? (
        <optgroup label={t("target.label")}>
          {targetPresets.map((preset) => (
            <option key={preset.id} value={preset.id}>
              {presetLabel(preset, t)}
            </option>
          ))}
        </optgroup>
      ) : null}
      {stylePresets.length > 0 ? (
        <optgroup label={t("presetSelector.styleGroup")}>
          {stylePresets.map((preset) => (
            <option key={preset.id} value={preset.id}>
              {presetLabel(preset, t)}
            </option>
          ))}
        </optgroup>
      ) : null}
    </SelectBox>
  );
}
