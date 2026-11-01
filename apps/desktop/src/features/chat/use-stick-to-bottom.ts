/**
 * 消息流「智能贴底」滚动（ChatGPT/Claude 对标）：
 * - 用户停在底部 → 新 token/新消息自动跟随滚动；
 * - 用户上滚阅读历史 → 不打断（不再强制拉底），改显「跳到最新」按钮；
 * - 判定纯函数 `isNearBottom` 抽出便于无头测试。
 */

import { useCallback, useEffect, useRef, useState, type RefObject } from "react";

/** 距底 `threshold`px 内视为"在底部"（滚动是亚像素/惯性的，严格等值判定会抖）。 */
export function isNearBottom(
  scrollTop: number,
  clientHeight: number,
  scrollHeight: number,
  threshold = 48,
): boolean {
  return scrollHeight - (scrollTop + clientHeight) <= threshold;
}

export function useStickToBottom(containerRef: RefObject<HTMLElement | null>, contentKey: unknown) {
  const [pinned, setPinned] = useState(true);
  const pinnedRef = useRef(true);

  const scrollToBottom = useCallback(
    (behavior: ScrollBehavior = "auto") => {
      const el = containerRef.current;
      if (!el) {
        return;
      }
      el.scrollTo({ top: el.scrollHeight, behavior });
      pinnedRef.current = true;
      setPinned(true);
    },
    [containerRef],
  );

  // 用户滚动 → 更新贴底状态（贴回底部即恢复跟随）。
  useEffect(() => {
    const el = containerRef.current;
    if (!el) {
      return;
    }
    function onScroll() {
      const target = containerRef.current;
      if (!target) {
        return;
      }
      const next = isNearBottom(target.scrollTop, target.clientHeight, target.scrollHeight);
      pinnedRef.current = next;
      setPinned(next);
    }
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      el.removeEventListener("scroll", onScroll);
    };
  }, [containerRef]);

  // 内容变化（新消息/流式 token）→ 仅在贴底时跟随。
  useEffect(() => {
    if (pinnedRef.current) {
      const el = containerRef.current;
      if (el) {
        el.scrollTop = el.scrollHeight;
      }
    }
  }, [containerRef, contentKey]);

  return { pinned, scrollToBottom };
}
