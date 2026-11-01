/**
 * 「这次的额外要求」（ADR-166 ⑤，PRD-19-042）：生成章节、渠道包装、分发之前补一句（可选）。
 *
 * 它与设置里的「我的频道风格」一起由 Rust 作为**内容**拼在提示词之后（`creator.notes.*`），
 * 不改提示词结构；长度上限也在 Rust 截（这里的 `maxLength` 只是同一个数字的输入提示）。
 * 不落盘：只管这一页的这几次生成。
 */

import { useTranslation } from "react-i18next";

/** 与 Rust `EXTRA_MAX_CHARS` 同一个数。 */
const EXTRA_MAX_CHARS = 500;

/** 发给命令的值：空白就是没有。 */
export function extraRequest(value: string): string | null {
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : null;
}

export interface ExtraRequestFieldProps {
  value: string;
  onChange: (value: string) => void;
}

export function ExtraRequestField({ value, onChange }: ExtraRequestFieldProps) {
  const { t } = useTranslation("workspace");
  return (
    <div className="luna-extra-request">
      <label className="luna-extra-request__field">
        <span className="luna-extra-request__label">{t("creator.extra.label")}</span>
        <input
          className="luna-input luna-input--sm"
          value={value}
          maxLength={EXTRA_MAX_CHARS}
          placeholder={t("creator.extra.placeholder")}
          onChange={(event) => onChange(event.target.value)}
        />
      </label>
      <span className="luna-extra-request__hint">{t("creator.extra.hint")}</span>
    </div>
  );
}
