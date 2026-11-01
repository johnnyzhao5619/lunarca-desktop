/**
 * 库里的附件有了 / 没了（PRD-06-012 ③，笔记库对标轮 V2b）。
 *
 * 库外删掉一张图（访达、同步盘、git），正文里引用它的地方要显示成断链；放回来又要重新显示。
 * Rust 的文件监听对附件也发 `vault://changed`（不进全文索引）。这里把一阵变更合成一个「附件
 * 代际号」：只要其中有一条不是笔记（不是 `.md`），代际号加一，订阅方据此清掉图片地址缓存、
 * 重新解析。
 *
 * **只看附件**：自动保存每次也发 `vault://changed`，若笔记变更也触发，正文里的每张图都会随每次
 * 保存重新解析、闪一下。笔记即 `.md` 文件是本仓的根约定，这里不是在判业务。
 */

import { useSyncExternalStore } from "react";

import { events } from "../../ipc/bindings";

/** 合并窗口：一次 `git checkout` 会带出成百个附件事件，只该刷新一次。 */
const DEBOUNCE_MS = 300;

let generation = 0;
const listeners = new Set<() => void>();
let unlisten: Promise<() => void> | null = null;
let timer = 0;

function ensureListening() {
  if (unlisten) return;
  unlisten = events.vaultChanged.listen((event) => {
    if (event.payload.rel_path.toLowerCase().endsWith(".md")) return;
    window.clearTimeout(timer);
    timer = window.setTimeout(() => {
      generation += 1;
      for (const listener of listeners) listener();
    }, DEBOUNCE_MS);
  });
  // 监听没挂上（不在 Tauri 里，例如编辑器单测挂出图片部件时）：不留半截状态，下次订阅再试；
  // 也不让它成为未处理的拒绝——那会把整轮测试判红。
  unlisten.catch(() => {
    unlisten = null;
  });
}

/** 订阅附件变化；返回退订函数。 */
export function onAssetsChanged(listener: () => void): () => void {
  ensureListening();
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** 当前附件代际号（编辑器图片部件据此判断要不要重建）。 */
export function currentAssetGeneration(): number {
  return generation;
}

/** React 组件里取附件代际号：变了就重新解析图片与嵌入。 */
export function useAssetGeneration(): number {
  return useSyncExternalStore(onAssetsChanged, currentAssetGeneration, currentAssetGeneration);
}
