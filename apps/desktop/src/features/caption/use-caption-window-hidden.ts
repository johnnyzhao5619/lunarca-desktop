/**
 * 字幕窗被收起时回调（`caption://window-hidden`，PRD-04-040 / 044）。
 *
 * 字幕窗常驻、收起只是 hide，组件不卸载、状态原样留着：收起时开着的外观面板，下一次打开
 * 还开着（真机 2026-10-02：「有时打开实时字幕会默认打开设置」）。译文窗还会被原文窗收起，
 * 它自己无从得知——所以收起的信号由 Rust 在 hide 处统一发出，两扇窗各认自己的 label。
 */

import { useEffect, useRef } from "react";
import { events } from "../../ipc/bindings";

export function useCaptionWindowHidden(label: string, onHidden: () => void) {
  const callback = useRef(onHidden);
  callback.current = onHidden;
  useEffect(() => {
    const unlisten = events.captionWindowHidden.listen((event) => {
      if (event.payload.label === label) callback.current();
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [label]);
}
