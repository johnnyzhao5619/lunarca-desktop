// 回收站条目预览模态（PRD-06-009：永久删除前判断内容，避免仅凭文件名误删）。
// 点击回收站某行唤起：文本笔记走共享 ReadingView 只读渲染（与阅读模式同一基线 DRY）、
// 文件夹列子项、二进制/媒体诚实降级为信息卡；底部就地「还原 / 永久删除」。业务无副作用，
// 内容经 read_trash_content 命令读取（.trash/<id>，前端零业务）。

import { useEffect, useState } from "react";
import { IconButton } from "../../components/ui/IconButton";
import { useTranslation } from "react-i18next";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { commands, type TrashEntryDto, type TrashPreviewDto } from "../../ipc/bindings";
import { ReadingView } from "../editor/ReadingView";
import { formatBytes } from "../../lib/format-bytes";
import {
  ArrowClockwiseIcon,
  CloseIcon,
  FolderIcon,
  HardDriveIcon,
  TrashIcon,
} from "../../components/ui/Icon";
import { formatDateTime } from "../../lib/format-datetime";
import { FileNodeIcon } from "./NoteTree";
import { vaultErrorText } from "../../lib/vault-error";

interface TrashPreviewProps {
  entry: TrashEntryDto;
  onClose: () => void;
  /** 还原当前条目（父级执行命令 + 刷新 + 关闭本模态）。 */
  onRestore: () => void;
  /** 永久删除当前条目（父级二次确认 + 命令 + 刷新 + 关闭）。 */
  onPurge: () => void;
}

export function TrashPreview({ entry, onClose, onRestore, onPurge }: TrashPreviewProps) {
  const { t, i18n } = useTranslation("workspace");
  const [preview, setPreview] = useState<TrashPreviewDto | null>(null);
  const [error, setError] = useState<string | null>(null);

  const name = entry.original_path.split("/").pop() ?? entry.original_path;
  const deletedAt = formatDateTime(entry.deleted_ms, i18n.language);

  useEffect(() => {
    let active = true;
    setPreview(null);
    setError(null);
    void commands.readTrashContent(entry.id).then((result) => {
      if (!active) {
        return;
      }
      if (result.status === "ok") {
        setPreview(result.data);
      } else {
        setError(vaultErrorText(result.error));
      }
    });
    return () => {
      active = false;
    };
  }, [entry.id]);

  // 模态行为（Esc / 焦点圈定 / 焦点归还）统一走共享 hook：此处此前只有一份手写的
  // Esc 监听，没有焦点圈定也没有焦点归还（十二张卡各写各的，能力参差）。
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose });

  function renderBody() {
    if (error) {
      return <p className="luna-trash-preview__status">{t("trashPreview.error", { error })}</p>;
    }
    if (!preview) {
      return <p className="luna-trash-preview__status">{t("trashPreview.loading")}</p>;
    }
    if (preview.kind === "note") {
      const content = preview.content ?? "";
      if (content.trim() === "") {
        return <p className="luna-trash-preview__status">{t("trashPreview.emptyNote")}</p>;
      }
      return (
        <>
          {preview.truncated && (
            <p className="luna-trash-preview__hint">{t("trashPreview.truncated")}</p>
          )}
          <div className="luna-trash-preview__note">
            <ReadingView content={content} notePath={null} />
          </div>
        </>
      );
    }
    if (preview.kind === "folder") {
      return (
        <div className="luna-trash-preview__folder">
          <p className="luna-trash-preview__folder-summary">
            <FolderIcon size={14} />
            <span>{t("trashPreview.folderCount", { count: preview.children.length })}</span>
          </p>
          {preview.truncated && (
            <p className="luna-trash-preview__hint">{t("trashPreview.folderTruncated")}</p>
          )}
          <ul className="luna-trash-preview__folder-list">
            {preview.children.map((child) => (
              <li key={child}>
                <FileNodeIcon path={child} className="luna-trash-preview__folder-icon" />
                <span>{child}</span>
              </li>
            ))}
          </ul>
        </div>
      );
    }
    // binary / media：诚实降级，不假装能预览。
    return (
      <div className="luna-trash-preview__binary">
        <HardDriveIcon size={20} />
        <div>
          <strong>{t("trashPreview.binaryTitle")}</strong>
          <small>
            {t("trashPreview.binarySize", { size: formatBytes(Number(preview.byte_size), i18n.language) })}
          </small>
        </div>
      </div>
    );
  }

  return (
    <div className="luna-overlay" role="presentation" onClick={onClose}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-dialog--panel luna-trash-preview"
        role="dialog"
        aria-modal="true"
        aria-label={name}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-trash-preview__header">
          <div className="luna-trash-preview__title">
            <FileNodeIcon path={entry.original_path} className="luna-trash-preview__title-icon" />
            <div className="luna-trash-preview__title-text">
              <strong>{name}</strong>
              <small>
                {t("trashPreview.meta", { path: entry.original_path, time: deletedAt })}
              </small>
            </div>
          </div>
          <IconButton
            label={t("trashPreview.close")}
            icon={<CloseIcon size={16} />}
            onClick={onClose}
          />
        </header>

        <div className="luna-trash-preview__body">{renderBody()}</div>

        <footer className="luna-trash-preview__actions">
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-trash__action"
            onClick={onRestore}
          >
            <ArrowClockwiseIcon size={14} />
            <span>{t("main.restoreFromTrash")}</span>
          </button>
          <button
            type="button"
            className="luna-btn luna-btn--danger-subtle luna-btn--sm luna-trash__action"
            onClick={onPurge}
          >
            <TrashIcon size={14} />
            <span>{t("main.purgeOne")}</span>
          </button>
        </footer>
      </div>
    </div>
  );
}
