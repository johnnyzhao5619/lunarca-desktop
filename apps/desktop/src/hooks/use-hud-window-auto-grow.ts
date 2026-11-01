import { useEffect, useRef, type RefObject } from "react";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { commands } from "../ipc/bindings";
import { nextHudWindowHeight, requiredHudContentHeight } from "./hud-window-layout";

type HudWindowAutoGrowOptions = {
  /**
   * 在隐藏态预热时也必须完整容纳的内容高度。
   *
   * 某些 flex 内容区在较矮首帧中会先被压缩，因此单看 scrollHeight 无法发现首帧裁切；
   * 此值只补足该真实最小内容高度，仍遵循“不缩小、只向下增长”的窗口所有权规则。
   */
  minContentHeight?: number;
  /**
   * 是否启用（默认启用）。`HudSurface` 统一挂本 Hook，而快捷对话与实时字幕是稳定
   * 可缩放窗口、不参与内容驱动扩容（TDD-07 §6）——Hook 必须无条件调用（React 规则），
   * 开关只能走参数。
   */
  enabled?: boolean;
  /**
   * 当前档位（ADR-112 ③）。档位变化 = 显式状态跃迁：交给 Rust `hud_set_tier` 按该档首帧高
   * `set_size`（可缩小）并做多屏 clamp；之后本 Hook 只在**档内**向下扩容。不传 = 无档位概念，
   * 只扩容不缩。
   */
  tier?: string;
};

/**
 * HUD 尺寸契约：窗口宽高一旦由用户调整就保持稳定；只有内容确实放不下时才向下扩容。
 *
 * 根元素必须填满当前 WebView。这里刻意不根据内容回写宽度，也不缩小窗口，避免
 * ResizeObserver 与用户拖拽争夺尺寸所有权。状态切换或窄窗换行导致内容增高时，
 * 仅把高度增长到可用屏幕范围；再大的正文由各 HUD 自己的内容区滚动承载。
 */
export function useHudWindowAutoGrow(
  ref: RefObject<HTMLElement | null>,
  { minContentHeight = 0, enabled = true, tier }: HudWindowAutoGrowOptions = {},
) {
  // 档位跃迁：只在档位**变化**时向 Rust 要该档几何（含收缩）；同档内容变化走下面的只扩容。
  const lastTierRef = useRef<string | undefined>(undefined);
  useEffect(() => {
    if (!enabled || tier === undefined || lastTierRef.current === tier) return;
    lastTierRef.current = tier;
    void commands.hudSetTier(tier).then((result) => {
      if (result.status === "error") {
        console.error("[useHudWindowAutoGrow] hud_set_tier failed:", result.error);
      }
    });
  }, [enabled, tier]);

  useEffect(() => {
    if (!enabled) return;
    const element = ref.current;
    if (!element) return;

    const appWindow = getCurrentWindow();
    let frame = 0;
    let disposed = false;
    let resizing = false;

    const measure = () => {
      frame = 0;
      if (disposed || resizing) return;

      const viewportWidth = Math.ceil(window.innerWidth);
      const viewportHeight = Math.ceil(window.innerHeight);
      // 不猜浏览器怎么算溢出：直接量最低一个后代盒子的底边 + 根的下内边距，与
      // scrollHeight 取大（`requiredHudContentHeight`）。此前按 scrollHeight 扩容，真机上译文
      // 动作条贴死窗口下缘（2026-09-07 两轮）。跳过不在布局流里的节点（隐藏 / fixed）。
      let contentBottom = 0;
      for (const node of element.querySelectorAll<HTMLElement>("*")) {
        if (node.offsetParent === null) continue;
        const bottom = node.getBoundingClientRect().bottom;
        if (bottom > contentBottom) contentBottom = bottom;
      }
      const paddingBottom = Number.parseFloat(getComputedStyle(element).paddingBlockEnd) || 0;
      const targetHeight = nextHudWindowHeight({
        viewportHeight,
        contentHeight: requiredHudContentHeight({
          scrollHeight: element.scrollHeight,
          contentBottom,
          paddingBottom,
        }),
        availableHeight: window.screen.availHeight,
        minContentHeight,
      });
      if (targetHeight === null) return;

      resizing = true;
      void appWindow
        .setSize(new LogicalSize(viewportWidth, targetHeight))
        .catch((error: unknown) => {
          console.error("[useHudWindowAutoGrow] failed to grow window:", error);
        })
        .finally(() => {
          resizing = false;
          // 尺寸变更在途期间到达的内容变化被 `resizing` 挡掉了（流式译文正是这种节奏），
          // 落定后补测一次，否则最后一截增量永远丢。
          schedule();
        });
    };

    const schedule = () => {
      if (disposed || frame !== 0) return;
      frame = window.requestAnimationFrame(measure);
    };

    const resizeObserver = new ResizeObserver(schedule);
    resizeObserver.observe(element);
    const mutationObserver = new MutationObserver(schedule);
    mutationObserver.observe(element, {
      attributes: true,
      characterData: true,
      childList: true,
      subtree: true,
    });
    window.addEventListener("resize", schedule);
    schedule();

    return () => {
      disposed = true;
      if (frame !== 0) window.cancelAnimationFrame(frame);
      resizeObserver.disconnect();
      mutationObserver.disconnect();
      window.removeEventListener("resize", schedule);
    };
  }, [enabled, minContentHeight, ref]);
}
