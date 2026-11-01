/**
 * 滚动区两端还有没有内容（「上面还有 / 下面还有」）：给隐藏了滚动条的窄区做渐隐提示用。
 *
 * 纯判定与订阅分开：判定可测；订阅只在滚动与尺寸变化时重算，内容增减由调用方经 `deps` 触发。
 */

import { useLayoutEffect, useState, type DependencyList, type RefObject } from "react";

export interface ScrollEdges {
  /** 上面还有被滚走的内容。 */
  start: boolean;
  /** 下面还有放不下的内容。 */
  end: boolean;
}

/** 亚像素取整误差（缩放 / 150% 字号下 scrollHeight 与 clientHeight 常差零点几）。 */
const EPSILON = 1;

export function scrollEdges(scrollTop: number, clientHeight: number, scrollHeight: number): ScrollEdges {
  return {
    start: scrollTop > EPSILON,
    end: scrollTop + clientHeight < scrollHeight - EPSILON,
  };
}

export function useScrollEdges(ref: RefObject<HTMLElement | null>, deps: DependencyList = []): ScrollEdges {
  const [edges, setEdges] = useState<ScrollEdges>({ start: false, end: false });
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    const update = () => {
      const next = scrollEdges(element.scrollTop, element.clientHeight, element.scrollHeight);
      setEdges((prev) => (prev.start === next.start && prev.end === next.end ? prev : next));
    };
    update();
    element.addEventListener("scroll", update, { passive: true });
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(update);
    observer?.observe(element);
    return () => {
      element.removeEventListener("scroll", update);
      observer?.disconnect();
    };
    // 内容增减不触发 scroll / resize：由调用方的 deps 驱动重量。
  }, [ref, ...deps]);
  return edges;
}
