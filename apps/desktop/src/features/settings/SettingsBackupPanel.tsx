/**
 * 设置备份与恢复（「数据 / 隐私」分组专属，PRD-09-050 / TDD-05 §7.4）。
 *
 * 导出 = 当前格式 TOML（设备级必带，可选附带当前库的 Vault 级），密钥由注册表 `secret`
 * 标志结构化排除；导入 = 选文件 → Rust 全量严格校验并**列出会改变的项**（预览，不写）→
 * 用户确认 → Rust 重读重验、与看过的清单核对后原子写回（2026-10-08 C46）。任何一处不过、
 * 或文件在两步之间变了，都不改现有配置。本面板只派发命令与显示结果。
 */

import { useState } from "react";
import type { commands as Commands } from "../../ipc/bindings";
import { useTranslation } from "react-i18next";
import { commands } from "../../ipc/bindings";
import { vaultErrorText } from "../../lib/vault-error";
import { InlineDetails } from "../../components/ui/InlineDetails";
import { FactRow } from "../../components/ui/FactRow";

/** 预览结果（生成物里是内联类型，这里取一个名字）。 */
type ImportPreview = NonNullable<
  Extract<Awaited<ReturnType<typeof Commands.previewSettingsImport>>, { status: "ok" }>["data"]
>;

export function SettingsBackupPanel() {
  const { t } = useTranslation("settings");
  const { t: tCommon } = useTranslation("common");
  const [includeVault, setIncludeVault] = useState(false);
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<string | null>(null);
  /** 已校验、待确认的导入（`null` = 没有在确认中）。 */
  const [pending, setPending] = useState<ImportPreview | null>(null);

  async function exportSettings() {
    setBusy(true);
    setFeedback(null);
    const result = await commands.exportSettings(includeVault);
    setBusy(false);
    if (result.status === "error") {
      setFeedback(vaultErrorText(result.error));
      return;
    }
    // null = 用户取消了保存对话框，不该报成功。
    if (result.data !== null) {
      setFeedback(t("settings_io.exported", { path: result.data }));
    }
  }

  function importError(error: string) {
    if (error.startsWith("SETTINGS_IMPORT_REJECTED")) return t("settings_io.rejected");
    if (error.startsWith("SETTINGS_IMPORT_PLAN_CHANGED")) return t("settings_io.plan_changed");
    return vaultErrorText(error);
  }

  async function previewImport() {
    setBusy(true);
    setFeedback(null);
    setPending(null);
    const result = await commands.previewSettingsImport();
    setBusy(false);
    if (result.status === "error") {
      setFeedback(importError(result.error));
      return;
    }
    // null = 用户取消了选择。
    if (result.data !== null) {
      setPending(result.data);
    }
  }

  async function confirmImport() {
    if (pending === null) return;
    setBusy(true);
    setFeedback(null);
    const result = await commands.importSettings(
      pending.path,
      pending.changes.map((change) => change.key),
    );
    setBusy(false);
    setPending(null);
    if (result.status === "error") {
      setFeedback(importError(result.error));
      return;
    }
    setFeedback(t("settings_io.imported", { device: result.data.device, vault: result.data.vault }));
  }

  return (
    <section
      id="settings-section-settings_io"
      tabIndex={-1}
      className="luna-settings-panel"
      aria-label={t("settings_io.label")}
    >
      <header className="luna-settings-panel__header">
        <h2>{t("settings_io.label")}</h2>
      </header>
      <p className="luna-settings-panel__hint">{t("settings_io.hint")}</p>
      {/* 作用范围与保护规则按需展开（2026-10-08 W12）：一句常显说清这是什么，细节不压在首屏。 */}
      <InlineDetails summary={t("settings_io.scope_summary")}>
        <p>{t("settings_io.scope")}</p>
      </InlineDetails>
      <label className="luna-settings-panel__toggle">
        <input
          type="checkbox"
          checked={includeVault}
          onChange={(event) => setIncludeVault(event.target.checked)}
        />
        <span>{t("settings_io.include_vault")}</span>
      </label>
      {/* 导入预览：校验已过，列出会改变的项；确认之前什么都没写。 */}
      {pending !== null ? (
        <div className="luna-fact-list" role="group" aria-label={t("settings_io.preview_title")}>
          {pending.changes.length === 0 ? (
            <p className="luna-fact-list__note">{t("settings_io.preview_none")}</p>
          ) : (
            pending.changes.map((change) => (
              <FactRow
                key={change.key}
                label={t(change.i18n_key)}
                value={t(`settings_io.scope_${change.scope}`, { defaultValue: change.scope })}
              />
            ))
          )}
          <p className="luna-fact-list__note">
            {t("settings_io.preview_summary", {
              count: pending.changes.length,
              unchanged: pending.unchanged,
            })}
          </p>
        </div>
      ) : null}
      <div className="luna-settings-panel__actions">
        <span className="luna-settings-panel__hint" role="status">
          {feedback}
        </span>
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          disabled={busy}
          onClick={() => void exportSettings()}
        >
          {t("settings_io.export")}
        </button>
        {pending === null ? (
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            disabled={busy}
            onClick={() => void previewImport()}
          >
            {t("settings_io.import")}
          </button>
        ) : (
          <>
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              disabled={busy}
              onClick={() => setPending(null)}
            >
              {tCommon("action.cancel")}
            </button>
            <button
              type="button"
              className="luna-btn luna-btn--primary"
              disabled={busy || pending.changes.length === 0}
              onClick={() => void confirmImport()}
            >
              {t("settings_io.confirm_import", { count: pending.changes.length })}
            </button>
          </>
        )}
      </div>
    </section>
  );
}
