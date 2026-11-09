/**
 * 标签改名 / 合并（ADR-135 / PRD-16-051）：新名字 → 预览影响 → 确认 → 逐篇回执 → 整批撤销。
 *
 * **前端零业务**：哪些笔记要改、是否其实是合并、哪些写法不支持而跳过，全由 Rust
 * `preview_tag_rename` 判定；确认时把预览里的每篇指纹原样带回，写入与撤销都在 Rust。
 * 名字合不合法也由 Rust 判——前端不另写一套标签字符集。
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  type TagEditChangeDto,
  type TagEditReceiptDto,
  type TagEditSkippedDto,
  type TagRenamePreviewDto,
  type TagUndoReceiptDto,
} from "../../ipc/bindings";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { IconButton } from "../../components/ui/IconButton";
import { CloseIcon } from "@lunarca/editor-core/Icon";
import { noteStem } from "../../lib/note-path";

type ReadyPreview = Extract<TagRenamePreviewDto, { status: "ready" }>;

type Stage =
  | { kind: "edit"; error: "invalid_name" | "same_tag" | null }
  | { kind: "loading" }
  | { kind: "preview"; preview: ReadyPreview }
  | { kind: "done"; receipt: TagEditReceiptDto; undo: TagUndoReceiptDto | null }
  | { kind: "failed" };

export interface TagRenameDialogProps {
  tag: string;
  onClose: () => void;
  /** 改名写成后：宿主据此把选中的标签跟到新名字上。 */
  onRenamed: (from: string, to: string) => void;
}

export function TagRenameDialog({ tag, onClose, onRenamed }: TagRenameDialogProps) {
  const { t } = useTranslation("workspace");
  const [name, setName] = useState(tag);
  const [stage, setStage] = useState<Stage>({ kind: "edit", error: null });
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose, enabled: true });

  const preview = () => {
    setStage({ kind: "loading" });
    void commands.previewTagRename(tag, name).then((result) => {
      if (result.status !== "ok") {
        setStage({ kind: "failed" });
      } else if (result.data.status === "ready") {
        setStage({ kind: "preview", preview: result.data });
      } else {
        setStage({ kind: "edit", error: result.data.status });
      }
    });
  };

  const apply = (ready: ReadyPreview) => {
    setStage({ kind: "loading" });
    void commands.applyTagRename(ready.from, ready.to, ready.targets).then((result) => {
      if (result.status !== "ok") {
        setStage({ kind: "failed" });
        return;
      }
      setStage({ kind: "done", receipt: result.data, undo: null });
      if (result.data.changed.length > 0) onRenamed(ready.from, ready.to);
    });
  };

  const undo = (receipt: TagEditReceiptDto, changed: TagEditChangeDto[]) => {
    void commands.undoTagRename(changed).then((result) => {
      if (result.status === "ok") {
        setStage({ kind: "done", receipt, undo: result.data });
        if (result.data.restored.length > 0) onRenamed(name.trim(), tag);
      }
    });
  };

  return (
    <div className="luna-overlay" role="presentation" onClick={onClose}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-topic-upgrade"
        role="dialog"
        aria-modal="true"
        aria-label={t("tagRename.title", { tag })}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-topic-upgrade__header">
          <h3>{t("tagRename.title", { tag })}</h3>
          <IconButton label={t("topicEvidence.close")} icon={<CloseIcon size={14} />} onClick={onClose} />
        </header>
        {stage.kind === "edit" && (
          <form
            className="luna-tag-rename__form"
            onSubmit={(event) => {
              event.preventDefault();
              preview();
            }}
          >
            <p className="luna-topic-upgrade__hint">{t("tagRename.hint")}</p>
            <input
              className="luna-input"
              value={name}
              aria-label={t("tagRename.newName")}
              autoFocus
              onChange={(event) => setName(event.target.value)}
            />
            {stage.error !== null && (
              <p className="luna-topic-upgrade__hint" role="alert">
                {t(`tagRename.error.${stage.error}`)}
              </p>
            )}
            <div className="luna-dialog__actions">
              <button type="button" className="luna-btn luna-btn--secondary" onClick={onClose}>
                {t("topicEvidence.close")}
              </button>
              <button type="submit" className="luna-btn luna-btn--primary">
                {t("tagRename.preview")}
              </button>
            </div>
          </form>
        )}
        {stage.kind === "loading" && (
          <p className="luna-empty" role="status">
            {t("common:status.loading")}
          </p>
        )}
        {stage.kind === "failed" && (
          <p className="luna-topic-upgrade__hint" role="alert">
            {t("tagRename.failed")}
          </p>
        )}
        {stage.kind === "preview" && (
          <PreviewBody
            preview={stage.preview}
            onBack={() => setStage({ kind: "edit", error: null })}
            onApply={() => apply(stage.preview)}
          />
        )}
        {stage.kind === "done" && (
          <DoneBody
            receipt={stage.receipt}
            undo={stage.undo}
            onUndo={() => undo(stage.receipt, stage.receipt.changed)}
            onClose={onClose}
          />
        )}
      </div>
    </div>
  );
}

function SkippedList({ skipped }: { skipped: TagEditSkippedDto[] }) {
  const { t } = useTranslation("workspace");
  if (skipped.length === 0) return null;
  return (
    <>
      <p className="luna-topic-upgrade__hint">{t("tagRename.skipped", { count: skipped.length })}</p>
      <ul className="luna-topic-upgrade__list">
        {skipped.map((item) => (
          <li key={item.path} className="luna-topic-upgrade__item" data-status="rejected">
            <span className="luna-topic-upgrade__page">{noteStem(item.path)}</span>
            <span className="luna-topic-upgrade__detail">{t(`tagRename.reason.${item.reason}`)}</span>
          </li>
        ))}
      </ul>
    </>
  );
}

function PreviewBody({
  preview,
  onBack,
  onApply,
}: {
  preview: ReadyPreview;
  onBack: () => void;
  onApply: () => void;
}) {
  const { t } = useTranslation("workspace");
  return (
    <>
      <p className="luna-topic-upgrade__hint">
        {t("tagRename.impact", { from: preview.from, to: preview.to, count: preview.targets.length })}
      </p>
      {preview.merge && (
        <p className="luna-topic-upgrade__hint" role="note">
          {t("tagRename.merge", { to: preview.to })}
        </p>
      )}
      {preview.targets.length > 0 && (
        <ul className="luna-topic-upgrade__list">
          {preview.targets.map((target) => (
            <li key={target.path} className="luna-topic-upgrade__item">
              <span className="luna-topic-upgrade__page">{noteStem(target.path)}</span>
            </li>
          ))}
        </ul>
      )}
      <SkippedList skipped={preview.skipped} />
      <div className="luna-dialog__actions">
        <button type="button" className="luna-btn luna-btn--secondary" onClick={onBack}>
          {t("tagRename.back")}
        </button>
        <button
          type="button"
          className="luna-btn luna-btn--primary"
          disabled={preview.targets.length === 0}
          onClick={onApply}
        >
          {t(preview.merge ? "tagRename.confirmMerge" : "tagRename.confirm", {
            count: preview.targets.length,
          })}
        </button>
      </div>
    </>
  );
}

function DoneBody({
  receipt,
  undo,
  onUndo,
  onClose,
}: {
  receipt: TagEditReceiptDto;
  undo: TagUndoReceiptDto | null;
  onUndo: () => void;
  onClose: () => void;
}) {
  const { t } = useTranslation("workspace");
  return (
    <>
      <p className="luna-topic-upgrade__hint" role="status">
        {undo === null
          ? t("tagRename.done", { count: receipt.changed.length })
          : t("tagRename.undone", { count: undo.restored.length })}
      </p>
      {undo === null && receipt.stale.length > 0 && (
        <p className="luna-topic-upgrade__hint">{t("tagRename.stale", { count: receipt.stale.length })}</p>
      )}
      {undo === null && receipt.failed.length > 0 && (
        <p className="luna-topic-upgrade__hint" role="alert">
          {t("tagRename.writeFailed", { count: receipt.failed.length })}
        </p>
      )}
      {undo === null && <SkippedList skipped={receipt.skipped} />}
      {undo !== null && undo.kept.length > 0 && (
        <p className="luna-topic-upgrade__hint">{t("tagRename.undoKept", { count: undo.kept.length })}</p>
      )}
      <div className="luna-dialog__actions">
        {undo === null && receipt.changed.length > 0 && (
          <button type="button" className="luna-btn luna-btn--secondary" onClick={onUndo}>
            {t("tagRename.undo")}
          </button>
        )}
        <button type="button" className="luna-btn luna-btn--primary" onClick={onClose}>
          {t("topicEvidence.close")}
        </button>
      </div>
    </>
  );
}
