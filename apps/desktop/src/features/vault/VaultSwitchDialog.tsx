/**
 * 切库护栏对话框（PRD-06-049）：阻断说清楚、代价先摆出来。
 *
 * 两种形态刻意不同：**阻断**（录音进行中）只给"知道了"——那是数据事故，不该给一条
 * "仍要切换"的路；**确认**（有活动任务 / 目标失效）如实说明代价后由用户决定。
 */

import { useTranslation } from "react-i18next";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import type { VaultSwitchController } from "./use-vault-switch";

export function VaultSwitchDialog({ controller }: { controller: VaultSwitchController }) {
  const { t } = useTranslation("workspace");
  const { state, confirm, dismiss } = controller;
  // 模态行为统一走共享 hook——本卡此前**完全没有 Esc**。`checking`/`switching` 是
  // 不可打断的在途态（对应下面的早退），故只在真正呈现出卡片时启用。
  const shown = state.kind !== "idle" && state.kind !== "checking" && state.kind !== "switching";
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose: dismiss, enabled: shown });

  if (!shown) {
    return null;
  }

  const body = () => {
    if (state.kind === "blocked") {
      return <p className="luna-dialog__note">{t("vault.blockedRecording")}</p>;
    }
    if (state.kind === "error") {
      return (
        <p className="luna-field-error" role="alert">
          {state.message}
        </p>
      );
    }
    return (
      <>
        {state.preflight.pending_tasks > 0 && (
          <p className="luna-dialog__note">
            {t("vault.confirmTasks", { count: state.preflight.pending_tasks })}
          </p>
        )}
        {state.preflight.target_missing && (
          <p className="luna-field-error" role="alert">
            {t("vault.confirmMissing")}
          </p>
        )}
      </>
    );
  };

  const canProceed = state.kind === "confirm" && !state.preflight.target_missing;

  return (
    <div className="luna-overlay" role="presentation" onClick={dismiss}>
      <div
        ref={dialogRef}
        className="luna-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={t("vault.switchTitle")}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-dialog__header">
          <h3>{t("vault.switchTitle")}</h3>
          {state.kind !== "error" && (
            <p className="luna-dialog__scope">{t("vault.switchTarget", { name: state.name })}</p>
          )}
        </header>
        {body()}
        <div className="luna-dialog__actions">
          <button type="button" className="luna-btn luna-btn--secondary" onClick={dismiss}>
            {canProceed ? t("vault.cancel") : t("vault.gotIt")}
          </button>
          {canProceed && (
            <button type="button" className="luna-btn luna-btn--primary" onClick={confirm}>
              {t("vault.switchAnyway")}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
