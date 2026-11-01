import { describe, expect, it } from "vitest";
import { intentOnOpen, shouldAutoplay } from "./autoplay";

const base = { loadedNote: "Recordings/a.md", loading: false, hasMedia: true };
const intent = { note: "Recordings/a.md", nonce: 1 };

describe("起播判据（PRD-04-064）", () => {
  it("有意图 + 装的就是它 + 装载完 + 有媒体 → 起播", () => {
    expect(shouldAutoplay({ ...base, intent })).toBe(true);
  });

  it("没有意图一律不起播——这条就是「切到录音/转写自动播放」那次回归的正解", () => {
    expect(shouldAutoplay({ ...base, intent: null })).toBe(false);
  });

  it("装的不是被要求的那一篇 → 等着，不放别人那篇", () => {
    expect(shouldAutoplay({ ...base, loadedNote: "Recordings/b.md", intent })).toBe(false);
  });

  it("还在装载 → 等", () => {
    expect(shouldAutoplay({ ...base, loading: true, intent })).toBe(false);
  });

  it("仅存链接的转写没有本地媒体 → 不起播", () => {
    expect(shouldAutoplay({ ...base, hasMedia: false, intent })).toBe(false);
  });

  it("空载时即便有意图也不起播", () => {
    expect(shouldAutoplay({ ...base, loadedNote: null, intent })).toBe(false);
  });
});

describe("open() 之后的意图（PRD-04-066 ②⑦）", () => {
  it("带 play → 指名这一篇、nonce 自增、seek 随意图走", () => {
    expect(intentOnOpen(intent, "Recordings/b.md", { play: true, seekMs: 1234 })).toEqual({
      note: "Recordings/b.md",
      nonce: 2,
      seekMs: 1234,
    });
    expect(intentOnOpen(null, "Recordings/a.md", { play: true })).toEqual({
      note: "Recordings/a.md",
      nonce: 1,
      seekMs: undefined,
    });
  });

  it("不带 play 换到别篇 → 旧意图作废（否则下次普通点开原篇会自动起播）", () => {
    expect(intentOnOpen(intent, "Recordings/b.md")).toBeNull();
  });

  it("不带 play 但还是同一篇 → 保留（回放视图挂载时会再 open 一次）", () => {
    expect(intentOnOpen(intent, "Recordings/a.md")).toBe(intent);
  });

  it("没有意图也没有 play → 仍然没有意图", () => {
    expect(intentOnOpen(null, "Recordings/a.md")).toBeNull();
  });
});
