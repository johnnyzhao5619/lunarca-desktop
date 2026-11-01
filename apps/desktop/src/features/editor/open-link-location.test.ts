// @vitest-environment happy-dom
/**
 * 点了带片段的库内链接（ADR-162 ⑭）：先打开那篇，再按 Rust 给的行与时刻去定位、起播。
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { EDITOR_REVEAL_EVENT, PLAYBACK_AT_EVENT } from "../../lib/app-events";
import { openLinkLocation } from "./open-link-location";

describe("openLinkLocation", () => {
  const seen: { type: string; detail: unknown }[] = [];
  const record = (event: Event) => seen.push({ type: event.type, detail: (event as CustomEvent).detail });
  window.addEventListener(EDITOR_REVEAL_EVENT, record);
  window.addEventListener(PLAYBACK_AT_EVENT, record);
  afterEach(() => {
    seen.length = 0;
  });

  it("带时刻：打开、到那一行、从那一刻播放", () => {
    const onOpenPath = vi.fn();
    openLinkLocation({ path: "访谈/访谈.md", line: 7, seek_ms: 83456 }, onOpenPath);
    expect(onOpenPath).toHaveBeenCalledWith("访谈/访谈.md");
    expect(seen).toEqual([
      { type: EDITOR_REVEAL_EVENT, detail: { path: "访谈/访谈.md", line: 7, headingIndex: null, focusEditor: true } },
      { type: PLAYBACK_AT_EVENT, detail: { path: "访谈/访谈.md", ms: 83456 } },
    ]);
  });

  it("没有片段：只打开", () => {
    const onOpenPath = vi.fn();
    openLinkLocation({ path: "Notes/a.md", line: null, seek_ms: null }, onOpenPath);
    expect(onOpenPath).toHaveBeenCalledWith("Notes/a.md");
    expect(seen).toEqual([]);
  });
});
