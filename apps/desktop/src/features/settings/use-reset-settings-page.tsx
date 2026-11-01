import { useRef } from "react";
import { useTranslation } from "react-i18next";
import { useConfirm } from "../../components/ConfirmHost";
import { FactRow } from "../../components/ui/FactRow";
import { commands, type SettingValueDto } from "../../ipc/bindings";
import type { SettingsController } from "./use-settings-controller";
import type { SettingsPageId } from "./settings-information-architecture";

/** 两个恢复入口共用 Rust 预览；确认时 Rust 在同一写锁内核对计划。 */
export function useResetSettingsPage(controller: SettingsController) {
  const { t } = useTranslation("settings");
  const ask = useConfirm();
  const current = useRef(controller);
  current.current = controller;
  const busy = useRef(false);
  const showValue = (key: string, value: SettingValueDto) => {
    if (value.kind === "bool") return t(value.value ? "action.value_enabled" : "action.value_disabled");
    const raw = Array.isArray(value.value) ? value.value.join(", ") : String(value.value);
    return t(`option.${key}.${raw}`, { defaultValue: raw || t("action.value_empty") });
  };
  return async (page: SettingsPageId) => {
    if (busy.current) return;
    busy.current = true;
    const scope = current.current.advancedMode ? "all" : "standard";
    const activePage = current.current.activePage;
    try {
      const result = await commands.previewSettingGroupReset(page, scope);
      if (result.status === "error" || result.data.changes.length === 0) {
        await ask({ title: t("action.reset_preview_unavailable"),
          description: t(result.status === "error" ? "action.reset_preview_failed" : "search.modified_empty"),
          confirmLabel: t("action.close"), destructive: false });
        return;
      }
      const plan = result.data;
      const label = scope === "all" ? "action.reset_group_all" : "action.reset_group_standard";
      const confirmed = await ask({
        title: t(`${label}_confirm`, { count: plan.changes.length }),
        description: t("action.reset_scope", { page: t(`group.${page}`),
          scope: t(scope === "all" ? "action.reset_scope_all" : "action.reset_scope_standard") }),
        confirmLabel: t(label),
        content: <div className="luna-settings-reset__preview">
          <div className="luna-fact-list">
            {plan.changes.map((spec) => <FactRow key={spec.key} label={t(spec.i18n_key)}
              value={`${showValue(spec.key, spec.current_value)} → ${showValue(spec.key, spec.default_value)}`}
              hint={spec.desc_presentation === "inline" ? t(spec.desc_i18n_key) : undefined} />)}
          </div>
          <p>{t("action.reset_effects")}</p>
        </div>,
      });
      if (!confirmed || current.current.activePage !== activePage
        || current.current.advancedMode !== (scope === "all")) return;
      await current.current.resetPage(page, scope, plan.fingerprint);
    } catch {
      await ask({ title: t("action.reset_preview_unavailable"),
        description: t("action.reset_preview_failed"), confirmLabel: t("action.close"), destructive: false });
    } finally { busy.current = false; }
  };
}
