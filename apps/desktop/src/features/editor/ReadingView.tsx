/**
 * 桌面的阅读视图（PRD-06-017）：共享阅读视图（`@lunarca/editor-core/ReadingView`，桌面与网页同一份）接上
 * 桌面的两样宿主端口——外链 / HTML / 复制按钮（`desktopMarkdownHost`）与库级能力（`desktopReadingVault`：
 * wiki 目标解析与打开、库内相对链接、嵌入、库内图片，全部经 IPC 问 Rust）。
 */

import type { ComponentProps } from "react";
import { ReadingView as SharedReadingView } from "@lunarca/editor-core/ReadingView";
import type { EmbedHit, ReadingVault } from "@lunarca/editor-core/reading-vault";
import { embedTargetsOf } from "@lunarca/editor-core/markdown";
import { prepareRichContent } from "@lunarca/editor-core/rich-content";
import { desktopMarkdownHost } from "../../components/markdown/markdown-host";
import { commands, type EmbedHitDto } from "../../ipc/bindings";
import { useAssetGeneration } from "../media/asset-changes";
import { vaultImageAssetSrc } from "../media/media-src";
import { openLinkLocation } from "./open-link-location";
import { openWikiLink, resolveWikiTargets } from "./wiki-link-open";

/** 桌面的库级能力：判定全在 Rust（ADR-003），这里只把阅读视图的问题转成 IPC。 */
export const desktopReadingVault: ReadingVault = {
  resolveWikiTargets,
  openWikiLink: (notePath, target, onOpenPath) => {
    void openWikiLink(notePath, target, { onOpenPath, allowCreate: false, onError: () => {} });
  },
  openVaultLink: (notePath, href, onOpenPath) => {
    void commands.vaultLinkLocation(notePath, href).then((result) => {
      if (result.status === "ok" && result.data) openLinkLocation(result.data, onOpenPath);
    });
  },
  resolveEmbeds: async (notePath, targets) => {
    const result = await commands.vaultResolveEmbeds(notePath, targets);
    return result.status === "ok" ? (result.data as EmbedHit[]) : null;
  },
  imageSource: async (notePath, src) => ({ url: await vaultImageAssetSrc(notePath, src) }),
  useAssetGeneration,
};

export type ReadingViewProps = Omit<ComponentProps<typeof SharedReadingView>, "host" | "vault" | "embeds"> & {
  /** 预先解析好的嵌入（导出 / 打印：静态渲染等不到异步结果）。缺省时自己问 Rust。 */
  embeds?: ReadonlyMap<string, EmbedHitDto>;
};

export function ReadingView(props: ReadingViewProps) {
  return <SharedReadingView {...props} host={desktopMarkdownHost} vault={desktopReadingVault} />;
}

/**
 * 一段 Markdown 的嵌入预先解析成表（导出、打印用；对标方案 D5「导出时展开」）：被嵌入笔记里的公式与
 * 图表一并排好——静态渲染只认已经排好的结果。被嵌入笔记里的嵌入不再解析。
 */
export async function resolveEmbedsFor(notePath: string, content: string): Promise<ReadonlyMap<string, EmbedHitDto>> {
  const targets = embedTargetsOf(content);
  if (targets.length === 0) return new Map();
  const result = await commands.vaultResolveEmbeds(notePath, targets);
  if (result.status !== "ok") return new Map();
  await Promise.all(result.data.map((hit) => (hit.kind === "note" ? prepareRichContent(hit.content) : undefined)));
  return new Map(targets.map((target, i) => [target, result.data[i]]));
}
