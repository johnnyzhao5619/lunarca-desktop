/**
 * 无边框窗口的「整条标题栏都是拖动把手」（HUD / 便签 / 独立笔记窗共用）。
 *
 * **判定必须认 `Element` 而不是 `HTMLElement`**（真机 2026-09-09）：图标按钮里那个
 * `<svg>` 是 `SVGElement`，它**不是** `HTMLElement`——按 `HTMLElement` 判就会在"点在图标
 * 的笔画上"时判成"点在空白处"，于是 `startDragging()` 接管指针，那一下的 `click` 根本
 * 不会发生。表现是**同一颗按钮，点在图标上没反应、点在它周围的内距上正常**，而且这
 * 事在 headless 里复现不出来（jsdom 没有原生拖动）。便签上的「在 Lunarca 中打开」就是
 * 这么坏的。`closest` 在 `Element` 上就有，改判据即可，不必给每个图标加
 * `pointer-events: none`（那是把同一件事分散到每处 CSS 里）。
 */

import { type PointerEvent } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

/** 这一下点在交互元素上（或它的任何后代，含 SVG 笔画）吗。 */
export function isInteractiveTarget(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) {
    return false;
  }
  return Boolean(
    target.closest(
      "button, textarea, input, select, a, [role='button'], [contenteditable='true'], [data-no-drag]",
    ),
  );
}

export function useWindowDrag() {
  const onPointerDown = (e: PointerEvent<HTMLElement>) => {
    // 仅响应鼠标左键，且避开交互元素
    if (e.button !== 0 || isInteractiveTarget(e.target)) {
      return;
    }

    e.preventDefault();

    // 调用 Tauri 原生 OS 拖动 API
    void getCurrentWindow().startDragging().catch((err) => {
      console.error("[useWindowDrag] failed to start native dragging:", err);
    });
  };

  return { onPointerDown };
}
