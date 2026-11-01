/**
 * 数据足迹与彻底清除（「数据 / 隐私」分组专属，PRD-09-026 / TDD-12 §11）。
 *
 * 一处列出 Lunarca 在这台机器上留下的全部数据位置与占用，并给分项清除：
 * - Vault 是用户的，只报不删；能清的是可重建缓存与回收站；
 * - 模型 / 凭据各一键清除（走与逐项相同的 Rust 路径）；
 * - 「重置应用」= 备份旧配置 + 设备级设置回默认 + 清应用缓存，之后重启；不碰 Vault、
 *   模型、凭据与本机记录（各有自己的入口，文案里指路）。
 * 全部事实与动作都在 Rust；本面板只渲染 DTO 与派发命令（ADR-003）。
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { HelpDetails } from "../../components/ui/HelpDetails";
import { InlineDetails } from "../../components/ui/InlineDetails";
import { PathValue } from "../../components/ui/PathDetails";
import { useConfirm } from "../../components/ConfirmHost";
import { commands, type FootprintEntryDto } from "../../ipc/bindings";
import { formatBytes } from "../../lib/format-bytes";
import { vaultErrorText } from "../../lib/vault-error";

type ClearAction = "vault_cache" | "vault_trash" | "models";

const CLEAR_ACTIONS: Partial<Record<string, ClearAction>> = {
  vault_cache: "vault_cache",
  vault_trash: "vault_trash",
  models: "models",
};

export function DataFootprintPanel() {
  const { t, i18n } = useTranslation("settings");
  const ask = useConfirm();
  const [entries, setEntries] = useState<FootprintEntryDto[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<string | null>(null);
  // 行级反馈：清除动作的结果就写在那一行的按钮旁——写在面板底部离得太远，真机上看起来
  // 像"点了没反应"（2026-09-06）。
  const [rowFeedback, setRowFeedback] = useState<{ id: string; text: string } | null>(null);

  const refresh = useCallback(async () => {
    const result = await commands.dataFootprint();
    setEntries(result.status === "ok" ? result.data.entries : null);
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  async function runClear(action: ClearAction) {
    const ok = await ask({
      title: t(`footprint.confirm.${action}_title`),
      description: t(`footprint.confirm.${action}`),
      confirmLabel: t(`footprint.confirm.${action}_action`),
      cancelLabel: t("footprint.cancel"),
    });
    if (!ok) {
      return;
    }
    setBusy(action);
    setFeedback(null);
    setRowFeedback(null);
    const say = (text: string) => setRowFeedback({ id: action, text });
    try {
      switch (action) {
        case "vault_cache": {
          const result = await commands.clearVaultCache();
          say(result.status === "ok" ? t("footprint.done.vault_cache") : vaultErrorText(result.error));
          break;
        }
        case "vault_trash": {
          const result = await commands.purge(null);
          say(result.status === "ok" ? t("footprint.done.vault_trash") : vaultErrorText(result.error));
          break;
        }
        case "models": {
          const result = await commands.clearAllLocalModels();
          say(
            result.status === "ok"
              ? t("footprint.done.models", { count: result.data })
              : vaultErrorText(result.error),
          );
          break;
        }
      }
    } finally {
      setBusy(null);
      void refresh();
    }
  }

  async function resetApp() {
    const ok = await ask({
      title: t("footprint.confirm.reset_title"),
      description: t("footprint.confirm.reset"),
      confirmLabel: t("footprint.confirm.reset_action"),
      cancelLabel: t("footprint.cancel"),
    });
    if (!ok) {
      return;
    }
    setBusy("reset");
    const result = await commands.resetApp();
    if (result.status === "error") {
      setBusy(null);
      setFeedback(vaultErrorText(result.error));
      return;
    }
    setFeedback(result.data ? t("footprint.done.reset", { path: result.data }) : t("footprint.done.reset_no_backup"));
    void commands.restartApp();
  }

  return (
    <section
      id="settings-section-footprint"
      tabIndex={-1}
      className="luna-settings-panel"
      aria-label={t("footprint.label")}
    >
      <header className="luna-settings-panel__header">
        <h2>{t("footprint.label")}</h2>
      </header>
      <p className="luna-settings-panel__hint">{t("footprint.hint")}</p>
      {entries === null ? (
        <p className="luna-settings-panel__hint">{t("footprint.loading")}</p>
      ) : (
        <div className="luna-settings-panel__rows">
          {entries.map((item) => {
            const action = CLEAR_ACTIONS[item.id];
            const value =
              item.count !== null && item.bytes === null
                ? t("footprint.count", { count: item.count })
                : item.bytes === null
                  ? "—"
                  : item.count !== null
                    ? t("footprint.bytes_and_count", { bytes: formatBytes(item.bytes, i18n.language), count: item.count })
                    : formatBytes(item.bytes, i18n.language);
            return (
              <div key={item.id} className="luna-settings-panel__row">
                <div className="luna-settings-panel__row-label">
                  <span className="luna-settings-panel__row-title">{t(`footprint.entry.${item.id}`)}
                    {item.id === "vault_cache" ? <HelpDetails summary={t("footprint.entry.vault_cache")}>
                      <p>{t("footprint.entry.vault_cache_desc")}</p>
                    </HelpDetails> : null}
                  </span>
                  {action ? <p className="luna-settings-panel__row-desc">{t(`footprint.impact.${action}`)}</p> : null}
                  {rowFeedback && rowFeedback.id === item.id ? (
                    <p className="luna-settings-panel__row-desc luna-settings-footprint__feedback" role="status">
                      {rowFeedback.text}
                    </p>
                  ) : null}
                </div>
                <span className="luna-settings-panel__row-value luna-settings-footprint__value">
                  {value}
                  {action && item.clearable ? (
                    <button
                      type="button"
                      className="luna-btn luna-btn--danger luna-btn--sm"
                      disabled={busy !== null}
                      onClick={() => void runClear(action)}
                    >
                      {t(`footprint.action.${action}`)}
                    </button>
                  ) : null}
                </span>
              </div>
            );
          })}
        </div>
      )}
      {entries?.some((item) => item.path !== null) ? <InlineDetails summary={t("footprint.locations")}>
        {entries.map((item) => item.path ? <div key={item.id}>
          <strong>{t(`footprint.entry.${item.id}`)}</strong>
          <PathValue path={item.path} />
        </div> : null)}
      </InlineDetails> : null}
      <InlineDetails summary={t("footprint.uninstall_title")}><p>{t("footprint.uninstall_hint")}</p></InlineDetails>
      <div className="luna-settings-panel__actions">
        <span className="luna-settings-panel__hint" role="status">
          {feedback}
        </span>
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          disabled={busy !== null}
          onClick={() => void refresh()}
        >
          {t("footprint.refresh")}
        </button>
        <button
          type="button"
          className="luna-btn luna-btn--danger"
          disabled={busy !== null}
          onClick={() => void resetApp()}
        >
          {t("footprint.action.reset")}
        </button>
      </div>
    </section>
  );
}
