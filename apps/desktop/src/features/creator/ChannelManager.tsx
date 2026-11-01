/**
 * 管理渠道（ADR-166 ⑦）：全部渠道按形态分组列出，每个可以勾「常用」（进「我的渠道」，各处选择
 * 渠道时只列这些）与「新项目默认」；自定义渠道在这里新建、修改、删除。
 *
 * 上限只显示 Rust 目录里有出处的那几个，没有的写「上限未核对」；发布时要做 AI 内容声明的平台
 * 标一个「发布须知」。存「我的渠道」走 `creator_set_channel_favorites`（Rust 归一化：认不出的丢掉、
 * 至少留一个），这里只渲染与发命令（ADR-003）。
 */

import { useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";

import { commands, type ChannelDto, type ChannelFavoriteDto, type ChannelLimitsDto } from "../../ipc/bindings";
import { useConfirm } from "../../components/ConfirmHost";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { vaultErrorText } from "../../lib/vault-error";
import { CHANNEL_FORMATS, ChannelEditor } from "./ChannelEditor";
import { channelName, reloadChannels, saveFavorites, useChannelFavorites, useChannels } from "./use-channels";

/** 上限摘要里的几项（顺序即显示顺序）。 */
const LIMIT_KEYS = ["title_max", "body_max", "tags_max", "tag_max", "clip_max_secs"] as const satisfies readonly (keyof ChannelLimitsDto)[];

export interface ChannelManagerProps {
  onClose: () => void;
}

export function ChannelManager({ onClose }: ChannelManagerProps) {
  const { t } = useTranslation("workspace");
  const catalog = useChannels();
  const favorites = useChannelFavorites();
  const confirm = useConfirm();
  /** 编辑器：`undefined` 关着，`null` 新建，否则改这一个（开着时管理框让开）。 */
  const [editing, setEditing] = useState<ChannelDto | null | undefined>(undefined);
  const [notice, setNotice] = useState<string | null>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose, initialFocusRef: closeRef });

  const favoriteOf = (id: string) => favorites.find((favorite) => favorite.id === id);

  const write = async (next: ChannelFavoriteDto[]) => {
    setNotice(null);
    const error = await saveFavorites(next);
    if (error) setNotice(vaultErrorText(error));
  };

  const toggleFavorite = (channel: ChannelDto) => {
    if (favoriteOf(channel.id)) {
      // 至少留一个：最后一个去不掉。
      if (favorites.length <= 1) return;
      void write(favorites.filter((favorite) => favorite.id !== channel.id));
      return;
    }
    void write([...favorites, { id: channel.id, default: false }]);
  };

  const toggleDefault = (channel: ChannelDto) => {
    void write(
      favorites.map((favorite) => (favorite.id === channel.id ? { ...favorite, default: !favorite.default } : favorite)),
    );
  };

  const remove = async (channel: ChannelDto) => {
    const ok = await confirm({
      title: t("creator.channels.deleteTitle", { name: channelName(channel, t) }),
      description: t("creator.channels.deleteHint"),
      confirmLabel: t("creator.channels.delete"),
    });
    if (!ok) return;
    const reply = await commands.creatorDeleteChannel(channel.id);
    if (reply.status !== "ok") {
      setNotice(vaultErrorText(reply.error));
      return;
    }
    reloadChannels();
  };

  const limitsText = (limits: ChannelLimitsDto) => {
    const parts = LIMIT_KEYS.flatMap((key) => {
      const value = limits[key];
      return value === null ? [] : [t(`creator.channels.limitShort.${key}`, { n: value })];
    });
    return parts.length > 0 ? parts.join(t("creator.channels.limitSeparator")) : t("creator.channels.unverified");
  };

  if (editing !== undefined) {
    return (
      <ChannelEditor
        channel={editing}
        onClose={() => setEditing(undefined)}
        onSaved={() => {
          setEditing(undefined);
          reloadChannels();
        }}
      />
    );
  }

  return createPortal(
    <div className="luna-overlay" role="presentation" onClick={onClose}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-channel-manager"
        role="dialog"
        aria-modal="true"
        aria-labelledby="channel-manager-title"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-dialog__header">
          <h3 id="channel-manager-title">{t("creator.channels.managerTitle")}</h3>
          <p>{t("creator.channels.managerHint")}</p>
        </header>
        <div className="luna-channel-manager__groups">
          {CHANNEL_FORMATS.map((format) => {
            const channels = catalog.filter((channel) => channel.format === format);
            if (channels.length === 0) return null;
            return (
              <section key={format} className="luna-channel-manager__group" aria-labelledby={`channel-format-${format}`}>
                <h4 id={`channel-format-${format}`}>{t(`creator.channelFormat.${format}`)}</h4>
                <ul className="luna-channel-manager__rows">
                  {channels.map((channel) => {
                    const favorite = favoriteOf(channel.id);
                    const name = channelName(channel, t);
                    return (
                      <li key={channel.id} className="luna-channel-manager__row">
                        <label className="luna-channel-manager__check">
                          <input
                            type="checkbox"
                            checked={favorite !== undefined}
                            disabled={favorite !== undefined && favorites.length <= 1}
                            onChange={() => toggleFavorite(channel)}
                          />
                          <span className="luna-channel-manager__name">{name}</span>
                        </label>
                        <span className="luna-channel-manager__limits">
                          {limitsText(channel.limits)}
                          {channel.ai_notice ? (
                            <span className="luna-channel-manager__notice" title={t(`creator.channelNotice.${channel.id}`)}>
                              {t("creator.channels.noticeBadge")}
                            </span>
                          ) : null}
                        </span>
                        <label className="luna-channel-manager__default">
                          <input
                            type="checkbox"
                            checked={favorite?.default ?? false}
                            disabled={favorite === undefined}
                            aria-label={t("creator.channels.defaultFor", { name })}
                            onChange={() => toggleDefault(channel)}
                          />
                          <span>{t("creator.channels.default")}</span>
                        </label>
                        {channel.builtin ? null : (
                          <span className="luna-channel-manager__actions">
                            <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={() => setEditing(channel)}>
                              {t("creator.channels.edit")}
                            </button>
                            <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={() => void remove(channel)}>
                              {t("creator.channels.delete")}
                            </button>
                          </span>
                        )}
                      </li>
                    );
                  })}
                </ul>
              </section>
            );
          })}
        </div>
        {notice ? (
          <p className="luna-channel-manager__error" role="alert">
            {notice}
          </p>
        ) : null}
        <div className="luna-dialog__actions">
          <button type="button" className="luna-btn luna-btn--secondary" onClick={() => setEditing(null)}>
            {t("creator.channels.new")}
          </button>
          <button ref={closeRef} type="button" className="luna-btn luna-btn--primary" onClick={onClose}>
            {t("creator.channels.done")}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
