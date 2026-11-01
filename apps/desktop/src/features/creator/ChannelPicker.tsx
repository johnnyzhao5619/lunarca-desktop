/**
 * 项目发到哪些渠道（ADR-166）：已选的渠道一枚一个胶囊（× 去掉，最后一个去不掉）。「添加渠道」
 * 先列「我的渠道」里还没加的，其余收在「其他渠道」里按形态分组（ADR-166 ⑦：不把全部平台都摊
 * 出来）；「管理渠道…」打开管理框，勾常用与新项目默认、增改删自定义渠道。
 *
 * 写回走 `creator_set_channels`（有序，写在项目主页）；渠道目录与判定在 Rust，这里只渲染。
 */

import { useState, type MouseEvent as ReactMouseEvent } from "react";
import { useTranslation } from "react-i18next";

import { commands, type ChannelDto } from "../../ipc/bindings";
import { useContextMenu } from "../../components/ContextMenuHost";
import { IconButton } from "../../components/ui/IconButton";
import { CloseIcon, PlusIcon } from "../../components/ui/Icon";
import { buildMenu, type ContextMenuEntry } from "../../lib/context-menu";
import { vaultErrorText } from "../../lib/vault-error";
import { CHANNEL_FORMATS } from "./ChannelEditor";
import { ChannelManager } from "./ChannelManager";
import {
  channelName,
  channelNameById,
  favoriteChannels,
  otherChannelsByFormat,
  useChannelFavorites,
  useChannels,
} from "./use-channels";

export interface ChannelPickerProps {
  home: string;
  /** 项目此刻的渠道（Rust 判定过的，有序）。 */
  channels: readonly string[];
  /** 写回之后：重判发布清单。 */
  onChanged: () => void;
}

export function ChannelPicker({ home, channels, onChanged }: ChannelPickerProps) {
  const { t } = useTranslation("workspace");
  const catalog = useChannels();
  const favorites = useChannelFavorites();
  const menu = useContextMenu();
  const [managing, setManaging] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  const write = async (next: string[]) => {
    setNotice(null);
    const reply = await commands.creatorSetChannels(home, next);
    if (reply.status !== "ok") {
      setNotice(vaultErrorText(reply.error));
      return;
    }
    onChanged();
  };

  const openAddMenu = (event: ReactMouseEvent) => {
    const add = (channel: ChannelDto) => ({
      kind: "action" as const,
      id: `channel_add_${channel.id}`,
      label: channelName(channel, t),
      onSelect: () => void write([...channels, channel.id]),
    });
    const mine = favoriteChannels(catalog, favorites).filter((channel) => !channels.includes(channel.id));
    const others = otherChannelsByFormat(
      catalog,
      new Set([...favorites.map((favorite) => favorite.id), ...channels]),
      CHANNEL_FORMATS,
    );
    const entries: (ContextMenuEntry | false)[] = [
      ...mine.map(add),
      others.length > 0 && {
        kind: "submenu",
        id: "channel_others",
        label: t("creator.channels.others"),
        items: others.map((group) => ({
          kind: "submenu" as const,
          id: `channel_group_${group.format}`,
          label: t(`creator.channelFormat.${group.format}`),
          items: group.channels.map(add),
        })),
      },
      { kind: "separator", id: "channel_sep" },
      { kind: "action", id: "channel_manage", label: t("creator.channels.manage"), onSelect: () => setManaging(true) },
    ];
    menu.open(event, buildMenu(entries));
  };

  return (
    <div className="luna-channel-picker" role="group" aria-label={t("creator.channels.label")}>
      <span className="luna-creator-page__label">{t("creator.channels.label")}</span>
      <ul className="luna-channel-picker__chips">
        {channels.map((id) => (
          <li key={id} className="luna-channel-picker__chip">
            <span>{channelNameById(id, catalog, t)}</span>
            <IconButton
              density="sm"
              icon={<CloseIcon size={10} />}
              label={t("creator.channels.remove", { name: channelNameById(id, catalog, t) })}
              disabled={channels.length <= 1}
              onClick={() => void write(channels.filter((channel) => channel !== id))}
            />
          </li>
        ))}
      </ul>
      <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" aria-haspopup="menu" onClick={openAddMenu}>
        <PlusIcon size={12} />
        {t("creator.channels.add")}
      </button>
      {notice ? (
        <span className="luna-creator-page__hint" role="alert">
          {notice}
        </span>
      ) : null}
      {managing ? <ChannelManager onClose={() => setManaging(false)} /> : null}
    </div>
  );
}
