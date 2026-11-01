/**
 * 新建 / 修改一个自定义发布渠道（ADR-166）：名字、形态、几个上限与一句风格说明。
 *
 * 上限留空 = 没有上限（或你也不确定）；保存走 `creator_save_channel`，名字的规矩（不能为空、
 * 不超过 40 字）由 Rust 判，这里只把它的说明就近显示。
 */

import { useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";

import { commands, type ChannelDto, type ChannelFormatDto, type ChannelLimitsDto } from "../../ipc/bindings";
import { SelectBox } from "../../components/ui/SelectBox";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { vaultErrorText } from "../../lib/vault-error";

/** 形态的顺序（与 Rust `ChannelFormat::ALL` 同序）。 */
export const CHANNEL_FORMATS: readonly ChannelFormatDto[] = [
  "long_video",
  "short_video",
  "article",
  "post",
  "newsletter",
  "audio",
];

/** 可填的几个上限（顺序即表单顺序）。 */
const LIMIT_FIELDS = ["title_max", "body_max", "tags_max", "tag_max", "clip_max_secs"] as const;
type LimitField = (typeof LIMIT_FIELDS)[number];

export interface ChannelEditorProps {
  /** 要改的那个自定义渠道；`null` = 新建。 */
  channel: ChannelDto | null;
  onSaved: (channel: ChannelDto) => void;
  onClose: () => void;
}

export function ChannelEditor({ channel, onSaved, onClose }: ChannelEditorProps) {
  const { t } = useTranslation("workspace");
  const [name, setName] = useState(channel?.name ?? "");
  const [format, setFormat] = useState<ChannelFormatDto>(channel?.format ?? "short_video");
  const [limits, setLimits] = useState<Record<LimitField, string>>(() => {
    const from = channel?.limits;
    return Object.fromEntries(LIMIT_FIELDS.map((field) => [field, from?.[field]?.toString() ?? ""])) as Record<
      LimitField,
      string
    >;
  });
  const [style, setStyle] = useState(channel?.style ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const nameRef = useRef<HTMLInputElement>(null);
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose, initialFocusRef: nameRef });

  const save = async () => {
    setBusy(true);
    setError(null);
    const parsed = Object.fromEntries(
      LIMIT_FIELDS.map((field) => {
        const value = Number.parseInt(limits[field], 10);
        return [field, Number.isFinite(value) && value > 0 ? value : null];
      }),
    ) as ChannelLimitsDto;
    const reply = await commands.creatorSaveChannel({
      id: channel?.id ?? null,
      name,
      format,
      limits: parsed,
      style,
    });
    setBusy(false);
    if (reply.status !== "ok") {
      setError(vaultErrorText(reply.error));
      return;
    }
    onSaved(reply.data);
  };

  return createPortal(
    <div className="luna-overlay" role="presentation" onClick={onClose}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-channel-editor"
        role="dialog"
        aria-modal="true"
        aria-labelledby="channel-editor-title"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-dialog__header">
          <h3 id="channel-editor-title">{t(channel ? "creator.channels.editTitle" : "creator.channels.newTitle")}</h3>
          <p>{t("creator.channels.editorHint")}</p>
        </header>
        <label className="luna-channel-editor__field">
          <span>{t("creator.channels.name")}</span>
          <input ref={nameRef} className="luna-input" value={name} onChange={(event) => setName(event.target.value)} />
        </label>
        <label className="luna-channel-editor__field">
          <span>{t("creator.channels.format")}</span>
          <SelectBox
            value={format}
            ariaLabel={t("creator.channels.format")}
            onChange={(event) => setFormat(event.currentTarget.value as ChannelFormatDto)}
          >
            {CHANNEL_FORMATS.map((value) => (
              <option key={value} value={value}>
                {t(`creator.channelFormat.${value}`)}
              </option>
            ))}
          </SelectBox>
        </label>
        <div className="luna-channel-editor__limits">
          {LIMIT_FIELDS.map((field) => (
            <label key={field} className="luna-channel-editor__field">
              <span>{t(`creator.channels.limit.${field}`)}</span>
              <input
                className="luna-input luna-input--sm"
                inputMode="numeric"
                value={limits[field]}
                placeholder={t("creator.channels.noLimit")}
                onChange={(event) => setLimits({ ...limits, [field]: event.target.value.replace(/\D/g, "") })}
              />
            </label>
          ))}
        </div>
        <label className="luna-channel-editor__field">
          <span>{t("creator.channels.style")}</span>
          <textarea
            className="luna-input"
            rows={3}
            value={style}
            placeholder={t("creator.channels.stylePlaceholder")}
            onChange={(event) => setStyle(event.target.value)}
          />
        </label>
        {error ? (
          <p className="luna-channel-editor__error" role="alert">
            {error}
          </p>
        ) : null}
        <div className="luna-dialog__actions">
          <button type="button" className="luna-btn luna-btn--secondary" onClick={onClose}>
            {t("common:action.cancel")}
          </button>
          <button type="button" className="luna-btn luna-btn--primary" disabled={busy || name.trim() === ""} onClick={() => void save()}>
            {t("creator.channels.save")}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
