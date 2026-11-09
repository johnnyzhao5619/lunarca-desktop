/**
 * 网页对阅读视图库级端口的实现（TDD-17 §2 / §5.5）：一切经 Worker 的命令表——链接指向哪一篇、嵌入指向
 * 什么由 WASM 按桌面同一份规则判定（`luna-sync-core::links`），图片在 Worker 里解密成 Blob URL，
 * 图片卸载时撤销。笔记嵌入在网页上照链接显示（Worker 不给正文，见 `resolve_embeds`）。
 */

import type { ReadingVault } from "@lunarca/editor-core/reading-vault";
import type { WorkerClient } from "../../worker/worker-client";

export function webReadingVault(client: WorkerClient): ReadingVault {
  return {
    resolveWikiTargets: (notePath, targets) =>
      client.call("resolve_wikilinks", { note_path: notePath, targets }).catch(() => null),
    openWikiLink: (notePath, target, onOpenPath) => {
      void client.call("resolve_wikilinks", { note_path: notePath, targets: [target] }).then(
        ([path]) => {
          if (path) onOpenPath(path);
        },
        () => {},
      );
    },
    openVaultLink: (notePath, href, onOpenPath) => {
      void client.call("resolve_link", { note_path: notePath, href }).then(
        (path) => {
          if (path) onOpenPath(path);
        },
        () => {},
      );
    },
    resolveEmbeds: (notePath, targets) =>
      client.call("resolve_embeds", { note_path: notePath, targets }).catch(() => null),
    imageSource: async (notePath, src) => {
      const url = await client.call("asset_url", { note_path: notePath, src });
      return { url, release: () => void client.call("release_asset", { url }).catch(() => {}) };
    },
    useAssetGeneration: () => 0,
  };
}
