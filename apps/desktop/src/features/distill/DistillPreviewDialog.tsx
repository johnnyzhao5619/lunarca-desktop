/**
 * 重生成前的差异预览（PRD-06-045 / ADR-024）。
 *
 * 「绝不覆盖用户资产」在此闭环：用户编辑过蒸馏笔记之后要重新整理，先看**会改掉什么**
 * 再决定要不要改。对比基准是**此刻磁盘上的内容**（含他的编辑），因为他要回答的问题
 * 正是"重新生成会改掉我现在看到的什么"（与版本对比 PRD-06-041 同一口径）。
 *
 * 接受时**不重跑 LLM**：Rust 侧暂存了预览产物，确认即落盘——各跑一次会让预览
 * 变成谎报（模型有随机性），那比没有预览更糟。
 */

import { useTranslation } from "react-i18next";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { DiffView } from "@lunarca/editor-core/DiffView";
import type { DistillPreviewController } from "./use-distill-preview";

export function DistillPreviewDialog({ controller }: { controller: DistillPreviewController }) {
  const { t } = useTranslation("workspace");
  const { state, close, apply } = controller;
  const open = state.kind !== "closed";

  // 模态行为（Esc / 焦点圈定 / 焦点归还）统一走共享 hook：此处此前只有一份手写的
  // Esc 监听，没有焦点圈定也没有焦点归还（十二张卡各写各的，能力参差）。
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose: close, enabled: open });

  if (state.kind === "closed") {
    return null;
  }

  const renderBody = () => {
    if (state.kind === "loading") {
      // 生成要跑一次本地 LLM（长录音可达分钟级）——如实说，不假装很快。
      return <p className="luna-dialog__note">{t("distill.previewRunning")}</p>;
    }
    if (state.kind === "error") {
      return (
        <p className="luna-field-error" role="alert">
          {state.message}
        </p>
      );
    }
    const { preview, applying } = state;
    const identical = preview.diff.added === 0 && preview.diff.removed === 0;
    return (
      <>
        <p className="luna-dialog__scope">{preview.target_path}</p>
        {preview.diff.degraded && (
          <p className="luna-dialog__note">{t("distill.previewDegraded")}</p>
        )}
        {identical ? (
          <p className="luna-dialog__note">{t("distill.previewIdentical")}</p>
        ) : (
          <p className="luna-dialog__note">
            {t("distill.previewCounts", {
              added: preview.diff.added,
              removed: preview.diff.removed,
            })}
          </p>
        )}
        <div className="luna-distill-preview__diff">
          <DiffView diff={preview.diff} />
        </div>
        <div className="luna-dialog__actions">
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            disabled={applying}
            onClick={close}
          >
            {t("distill.previewDiscard")}
          </button>
          <button
            type="button"
            className="luna-btn luna-btn--primary"
            disabled={applying || identical}
            onClick={() => void apply()}
          >
            {t("distill.previewApply")}
          </button>
        </div>
      </>
    );
  };

  return (
    <div className="luna-overlay" role="presentation" onClick={close}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-distill-preview"
        role="dialog"
        aria-modal="true"
        aria-label={t("distill.previewTitle")}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-dialog__header">
          <h3>{t("distill.previewTitle")}</h3>
          <p>{t("distill.previewDesc")}</p>
        </header>
        {renderBody()}
      </div>
    </div>
  );
}
