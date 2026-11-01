/**
 * 「整理列表层级」确认卡（PRD-06-056）。
 *
 * 显式动作的可见证据：改动清单由 Rust 计算（归一规则单点在 `luna_vault::tidy`，
 * ADR-003 前端零业务），本卡只负责把「将改动哪几行、改成什么样」逐行摆出来——
 * 用户没看到 before/after 之前，一个字节都不会写盘（绝不静默改用户文件）。
 * 确认卡范式循 `DistillBatchDialog` / `GraphRescanDialog`（先摆代价，再给一个开始按钮）。
 */

import { useTranslation } from "react-i18next";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import type { TidyListPreviewDto } from "../../ipc/bindings";

export function TidyListDialog({
  preview,
  onCancel,
  onConfirm,
}: {
  preview: TidyListPreviewDto;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const { t } = useTranslation("editor");
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose: onCancel });

  return (
    <div className="luna-overlay" role="presentation" onClick={onCancel}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-tidy-list"
        role="dialog"
        aria-modal="true"
        aria-label={t("tidyList.title")}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-dialog__header">
          <h3>{t("tidyList.title")}</h3>
          <p>{t("tidyList.desc", { count: preview.changes.length })}</p>
        </header>
        <ul className="luna-tidy-list__rows">
          {preview.changes.map((change) => (
            <li key={change.line} className="luna-tidy-list__row">
              <span className="luna-tidy-list__line">
                {t("tidyList.lineNo", { line: change.line })}
              </span>
              <span className="luna-tidy-list__before">{change.before}</span>
              <span className="luna-tidy-list__arrow" aria-hidden="true">
                →
              </span>
              <span className="luna-tidy-list__after">{change.after}</span>
            </li>
          ))}
        </ul>
        {/* 写盘前照例留版本快照（TDD-06 §4.7）：改错了可从版本历史找回。 */}
        <p className="luna-dialog__note">{t("tidyList.snapshotNote")}</p>
        <div className="luna-dialog__actions">
          <button type="button" className="luna-btn luna-btn--secondary" onClick={onCancel}>
            {t("tidyList.cancel")}
          </button>
          <button type="button" className="luna-btn luna-btn--primary" onClick={onConfirm}>
            {t("tidyList.confirm", { count: preview.changes.length })}
          </button>
        </div>
      </div>
    </div>
  );
}
