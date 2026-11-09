/**
 * 模型安装前的**确认卡**（PRD-10-015 / PRD-14-020）：下载体积、装机后真实占用、完成后
 * 磁盘剩余、许可，以及被拦截时的原因——模型库与首启引导**同一张**。
 *
 * 两处各画一张的话，迟早一边少一行（比如装机占用），而那正是 PRD-10-015 要求诚实的那一行。
 */

import { useTranslation } from "react-i18next";
import type { ModelInstallPlanDto } from "../../ipc/bindings";
import { formatBytes } from "@lunarca/editor-core/format-bytes";

export function ModelInstallPlanCard({
  plan,
  busy,
  onConfirm,
  onCancel,
}: {
  plan: ModelInstallPlanDto;
  /** 正在开始安装（按钮禁用、文案不变——进度由调用方的进度条呈现）。 */
  busy?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const { t, i18n } = useTranslation("settings");
  return (
    <div
      className="luna-settings-model__plan"
      role="group"
      aria-label={t(plan.replaces_installed ? "models.plan.update_title" : "models.plan.install_title")}
    >
      <strong>{t(plan.replaces_installed ? "models.plan.update_title" : "models.plan.install_title")}</strong>
      {/* 更新从不静默替换（ADR-145 ④）：这张卡就是那次确认，替换与重建索引都要当场说清。 */}
      {plan.replaces_installed ? <span>{t("models.plan.update_note")}</span> : null}
      {plan.requires_reindex ? (
        <span className="luna-field-error" role="note">
          {t("models.plan.reindex_note")}
        </span>
      ) : null}
      {plan.resumed_bytes !== null ? (
        <span>
          {t("models.plan.download_resumed", {
            value: formatBytes(plan.download_bytes, i18n.language),
            done: formatBytes(plan.resumed_bytes, i18n.language),
          })}
        </span>
      ) : (
        <span>{t("models.plan.download", { value: formatBytes(plan.download_bytes, i18n.language) })}</span>
      )}
      <span>{t("models.plan.on_disk", { value: formatBytes(plan.estimated_on_disk_bytes, i18n.language) })}</span>
      {plan.remaining_after_bytes !== null ? (
        <span>{t("models.plan.remaining", { value: formatBytes(plan.remaining_after_bytes, i18n.language) })}</span>
      ) : null}
      <span>{t("models.plan.license", { value: plan.license_label })}</span>
      {plan.blocking_reason ? (
        <span className="luna-field-error" role="alert">
          {t(`models.plan.reason.${plan.blocking_reason}`)}
        </span>
      ) : null}
      <div className="luna-settings-model__actions">
        <button
          type="button"
          className="luna-btn luna-btn--primary"
          disabled={!plan.can_start || busy}
          onClick={onConfirm}
        >
          {t(plan.replaces_installed ? "models.plan.update_confirm" : "models.plan.install_confirm")}
        </button>
        <button type="button" className="luna-btn luna-btn--secondary" onClick={onCancel}>
          {t("models.plan.cancel")}
        </button>
      </div>
    </div>
  );
}
