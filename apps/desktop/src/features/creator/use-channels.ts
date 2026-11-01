/**
 * 发布渠道目录（ADR-166）：内置渠道 + 这个库里的自定义渠道，由 Rust 一次给齐（`creator_channels`），
 * 以及「我的渠道」（`creator_channel_favorites`：各处选择渠道时只列这些，`default` = 新项目默认发）。
 * 每个窗口一份模块级快照，按订阅者引用计数；改了自定义渠道或「我的渠道」之后调 `reloadChannels()`
 * （存「我的渠道」直接用 `saveFavorites`，回来的就是 Rust 归一化后的那一份）。
 *
 * 渠道名：内置渠道按 id 取译名（`creator.channel.<id>`），自定义渠道用用户起的名字；认不出的 id
 * （自定义渠道在别的设备上被删了）原样显示 id，不隐藏——项目主页里还记着它。
 */

import { useSyncExternalStore } from "react";
import type { TFunction } from "i18next";

import { commands, type ChannelDto, type ChannelFavoriteDto, type ChannelFormatDto } from "../../ipc/bindings";

interface ChannelsSnapshot {
  catalog: readonly ChannelDto[];
  favorites: readonly ChannelFavoriteDto[];
}

let snapshot: ChannelsSnapshot = { catalog: [], favorites: [] };
const listeners = new Set<() => void>();

function publish(next: Partial<ChannelsSnapshot>) {
  snapshot = { ...snapshot, ...next };
  for (const listener of listeners) listener();
}

/** 重取一次目录与「我的渠道」（自定义渠道改动之后）。 */
export function reloadChannels(): void {
  void commands.creatorChannels().then((reply) => {
    if (reply.status === "ok") publish({ catalog: reply.data });
  });
  void commands.creatorChannelFavorites().then((reply) => {
    if (reply.status === "ok") publish({ favorites: reply.data });
  });
}

/** 存「我的渠道」：成功时快照换成 Rust 归一化后的那一份；失败返回错误串。 */
export async function saveFavorites(favorites: ChannelFavoriteDto[]): Promise<string | null> {
  const reply = await commands.creatorSetChannelFavorites(favorites);
  if (reply.status !== "ok") return reply.error;
  publish({ favorites: reply.data });
  return null;
}

function subscribe(onChange: () => void): () => void {
  if (listeners.size === 0) reloadChannels();
  listeners.add(onChange);
  return () => {
    listeners.delete(onChange);
  };
}

/** 渠道目录（内置在前、自定义在后）；取回之前为空。 */
export function useChannels(): readonly ChannelDto[] {
  return useSyncExternalStore(subscribe, () => snapshot.catalog);
}

/** 「我的渠道」（按存的顺序）；取回之前为空。 */
export function useChannelFavorites(): readonly ChannelFavoriteDto[] {
  return useSyncExternalStore(subscribe, () => snapshot.favorites);
}

/** 一个渠道给人看的名字。 */
export function channelName(channel: ChannelDto, t: TFunction): string {
  return channel.builtin ? t(`workspace:creator.channel.${channel.id}`) : (channel.name ?? channel.id);
}

/** 按 id 取名字（目录里没有就显示 id 本身）。 */
export function channelNameById(id: string, catalog: readonly ChannelDto[], t: TFunction): string {
  const channel = catalog.find((entry) => entry.id === id);
  return channel ? channelName(channel, t) : id;
}

/** 「我的渠道」按存的顺序取成目录项（目录里没有的跳过）。 */
export function favoriteChannels(
  catalog: readonly ChannelDto[],
  favorites: readonly ChannelFavoriteDto[],
): ChannelDto[] {
  return favorites.flatMap((favorite) => catalog.filter((channel) => channel.id === favorite.id));
}

/** 不在「我的渠道」里的那些，按形态分组（「其他渠道」菜单）；没有渠道的形态不出现。 */
export function otherChannelsByFormat(
  catalog: readonly ChannelDto[],
  exclude: ReadonlySet<string>,
  formats: readonly ChannelFormatDto[],
): { format: ChannelFormatDto; channels: ChannelDto[] }[] {
  return formats
    .map((format) => ({
      format,
      channels: catalog.filter((channel) => channel.format === format && !exclude.has(channel.id)),
    }))
    .filter((group) => group.channels.length > 0);
}
