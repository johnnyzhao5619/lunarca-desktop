/**
 * 后台起草新主题（PRD-16-060）落定时通知宿主重取：初稿只写进 `.luna/sidecars`，
 * `vault://changed` 不会来，而建议行上的「初稿已备好 · N 条」正依赖它。
 *
 * 跟着队列广播看「还有没有在跑的 `topic_draft_prepare`」：从有到无即重取一次。只作触发，
 * 有没有备好、备了几条仍以 Rust 的建议为准（ADR-003）。
 */

import { useEffect, useRef } from "react";
import { events } from "../../ipc/bindings";
import { isTaskActive } from "../processing/processing-labels";

export function useTopicDraftsSettled(onSettled: () => void): void {
  const callbackRef = useRef(onSettled);
  callbackRef.current = onSettled;
  useEffect(() => {
    let running = false;
    const unlisten = events.processingQueue.listen((event) => {
      const active = event.payload.items.some(
        (task) => task.kind === "topic_draft_prepare" && isTaskActive(task),
      );
      if (running && !active) callbackRef.current();
      running = active;
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, []);
}
