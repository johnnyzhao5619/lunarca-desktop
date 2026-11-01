/**
 * 新建项目对话框（ADR-165 评审遗留 S6c）：项目名、这期发到哪些渠道、计划发布日期，一次填完。
 *
 * 渠道：只列「我的渠道」（ADR-166 ⑦），初值是其中标了新项目默认的（`creator_default_channels`），
 * 可增减、至少留一个；别的渠道在「其他渠道」菜单里按形态找。日期可不填。建项目、校验名字与日期
 * 都在 Rust（`creator_create_project`），这里只收集。
 */

import { useEffect, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";

import { commands, type ChannelDto, type NewProjectOptionsDto } from "../../ipc/bindings";
import { useContextMenu } from "../../components/ContextMenuHost";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { buildMenu } from "../../lib/context-menu";
import { isImeKeyEvent } from "../../lib/ime-key-event";
import { CHANNEL_FORMATS } from "./ChannelEditor";
import { channelName, favoriteChannels, otherChannelsByFormat, useChannelFavorites, useChannels } from "./use-channels";

export interface NewProjectDialogProps {
  /** 点「新建」：交给宿主去建（失败原因由宿主写回 `error`）。 */
  onCreate: (name: string, options: NewProjectOptionsDto) => void;
  onClose: () => void;
  busy: boolean;
  error: string | null;
}

export function NewProjectDialog({ onCreate, onClose, busy, error }: NewProjectDialogProps) {
  const { t } = useTranslation("workspace");
  const catalog = useChannels();
  const favorites = useChannelFavorites();
  const menu = useContextMenu();
  const [name, setName] = useState("");
  const [channels, setChannels] = useState<string[] | null>(null);
  const [date, setDate] = useState("");
  const nameRef = useRef<HTMLInputElement>(null);
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose, initialFocusRef: nameRef });

  useEffect(() => {
    let live = true;
    void commands.creatorDefaultChannels().then((reply) => {
      if (live && reply.status === "ok") setChannels((current) => current ?? reply.data);
    });
    return () => {
      live = false;
    };
  }, []);

  const picked = channels ?? [];
  const toggle = (id: string) => {
    // 至少留一个：最后一个按不掉。
    if (picked.includes(id)) {
      if (picked.length > 1) setChannels(picked.filter((channel) => channel !== id));
      return;
    }
    setChannels([...picked, id]);
  };

  /** 胶囊：「我的渠道」，再加上这次从「其他渠道」里挑进来的。 */
  const mine = favoriteChannels(catalog, favorites);
  const shown: ChannelDto[] = [
    ...mine,
    ...picked.flatMap((id) => (mine.some((channel) => channel.id === id) ? [] : catalog.filter((channel) => channel.id === id))),
  ];

  const openOthers = (event: ReactMouseEvent) => {
    const groups = otherChannelsByFormat(catalog, new Set(shown.map((channel) => channel.id)), CHANNEL_FORMATS);
    menu.open(
      event,
      buildMenu(
        groups.map((group) => ({
          kind: "submenu" as const,
          id: `new_project_group_${group.format}`,
          label: t(`creator.channelFormat.${group.format}`),
          items: group.channels.map((channel) => ({
            kind: "action" as const,
            id: `new_project_add_${channel.id}`,
            label: channelName(channel, t),
            onSelect: () => toggle(channel.id),
          })),
        })),
      ),
    );
  };

  const submit = () => {
    if (busy || name.trim() === "") return;
    onCreate(name.trim(), { channels: channels, publish_date: date === "" ? null : date });
  };

  return createPortal(
    <div className="luna-overlay" role="presentation" onClick={onClose}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-new-project"
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-project-title"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-dialog__header">
          <h3 id="new-project-title">{t("creator.project.newTitle")}</h3>
          <p>{t("creator.project.newDescription")}</p>
        </header>
        <label className="luna-new-project__field">
          <span>{t("creator.project.nameLabel")}</span>
          <input
            ref={nameRef}
            className="luna-input"
            value={name}
            onChange={(event) => setName(event.target.value)}
            onKeyDown={(event) => {
              // 输入法上屏那一下的回车不算提交（全应用唯一判据）。
              if (event.key === "Enter" && !isImeKeyEvent(event)) submit();
            }}
          />
        </label>
        <div className="luna-new-project__field">
          <span id="new-project-channels">{t("creator.project.newChannels")}</span>
          <div className="luna-creator-page__choices" role="group" aria-labelledby="new-project-channels">
            {shown.map((channel) => (
              <button
                key={channel.id}
                type="button"
                className="luna-creator-page__choice"
                aria-pressed={picked.includes(channel.id)}
                onClick={() => toggle(channel.id)}
              >
                {channelName(channel, t)}
              </button>
            ))}
            <button type="button" className="luna-creator-page__choice" aria-haspopup="menu" onClick={openOthers}>
              {t("creator.project.newOtherChannels")}
            </button>
          </div>
        </div>
        <label className="luna-new-project__field">
          <span>{t("creator.project.newDate")}</span>
          <input type="date" className="luna-input" value={date} onChange={(event) => setDate(event.target.value)} />
        </label>
        {error ? (
          <p className="luna-new-project__error" role="alert">
            {error}
          </p>
        ) : null}
        <div className="luna-dialog__actions">
          <button type="button" className="luna-btn luna-btn--secondary" onClick={onClose}>
            {t("common:action.cancel")}
          </button>
          <button
            type="button"
            className="luna-btn luna-btn--primary"
            disabled={busy || name.trim() === ""}
            onClick={submit}
          >
            {t("creator.project.create")}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
