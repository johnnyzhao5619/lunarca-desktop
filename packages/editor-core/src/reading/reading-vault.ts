/**
 * 阅读视图的库级能力端口（TDD-17 §2）：与具体一个库相关的事——wiki 目标指向哪一篇、点开之后怎么走、
 * 库内相对链接打开哪一篇、嵌入指向什么、库内图片的地址——都交宿主。桌面经 IPC 问 Rust，网页经
 * Worker 问 WASM；两边的判定都在 Rust 里，阅读视图只负责呈现。
 */

/** 嵌入 `![[…]]` 指向什么（与桌面 `EmbedHitDto` 同形）。 */
export type EmbedHit =
  | { kind: "missing" }
  | { kind: "file"; path: string; link: string; image: boolean }
  | { kind: "note"; path: string; content: string };

/** 一张库内图片：可显示的地址，与用完之后的释放（网页撤销 Blob URL；桌面不需要）。 */
export interface VaultImageSource {
  url: string;
  release?: () => void;
}

export interface ReadingVault {
  /** 一批 wiki 目标（`|` 之前的原文）各指向哪一篇；`null` 项 = 指不到。整体失败回 `null`（保持原样）。 */
  resolveWikiTargets(notePath: string, targets: string[]): Promise<(string | null)[] | null>;
  /** 只读阅读面上点一条 wiki-link：打开指得到的那一篇，不新建。 */
  openWikiLink(notePath: string, target: string, onOpenPath: (path: string) => void): void;
  /** 库内相对链接（`[标题](../甲.md)`）：解析与定位归宿主，指不到就什么都不做。 */
  openVaultLink(notePath: string, href: string, onOpenPath: (path: string) => void): void;
  /** 一批嵌入目标各指向什么；`null` 项 = 先照链接显示。整体失败回 `null`。 */
  resolveEmbeds(notePath: string, targets: string[]): Promise<(EmbedHit | null)[] | null>;
  /** 库内图片（笔记相对地址）→ 可显示的地址；指不到就抛错，阅读面显示占位。 */
  imageSource(notePath: string, src: string): Promise<VaultImageSource>;
  /**
   * 库外改了附件之后让图片与嵌入重新解析（桌面监听文件变化）。React hook：宿主没有这件事就恒返回 0。
   */
  useAssetGeneration(): number;
}
