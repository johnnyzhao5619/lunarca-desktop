import { describe, expect, it } from "vitest";
import type { SpeakerStatDto } from "../../ipc/bindings";
import { SPEAKER_SLOT_COUNT, speakerDisplayName, speakerSlot } from "./speaker-view";

const labels = {
  self: "我",
  remote: "对方",
  remoteN: (n: string) => `对方${n}`,
  speakerN: (n: string) => `说话人${n}`,
};

function roster(...ids: string[]): SpeakerStatDto[] {
  return ids.map((id) => ({ id, segment_count: 1, duration_ms: 1000 }));
}

describe("speakerDisplayName", () => {
  it("改名映射优先于默认轨标签", () => {
    expect(speakerDisplayName("remote:1", { "remote:1": "张三" }, labels)).toBe("张三");
    expect(speakerDisplayName("self", { self: "  " }, labels), "空白改名视同未改名").toBe("我");
  });

  it("无改名时按 id 家族给默认标签", () => {
    expect(speakerDisplayName("self", {}, labels)).toBe("我");
    expect(speakerDisplayName("remote", {}, labels)).toBe("对方");
    expect(speakerDisplayName("remote:2", {}, labels)).toBe("对方2");
    expect(speakerDisplayName("speaker:3", {}, labels)).toBe("说话人3");
  });

  it("未知 id 原样兜底（绝不猜测）", () => {
    expect(speakerDisplayName("alien", {}, labels)).toBe("alien");
    expect(speakerDisplayName("remote:x", {}, labels)).toBe("remote:x");
  });
});

describe("speakerSlot", () => {
  it("槽位取名册下标（首次出现序），同一录音内稳定", () => {
    const list = roster("self", "remote:1", "remote:2");
    expect(speakerSlot("self", list)).toBe(1);
    expect(speakerSlot("remote:1", list)).toBe(2);
    expect(speakerSlot("remote:2", list)).toBe(3);
  });

  it("超出槽位数即循环复用", () => {
    const list = roster("a", "b", "c", "d", "e", "f", "g");
    expect(speakerSlot("f", list)).toBe(SPEAKER_SLOT_COUNT);
    expect(speakerSlot("g", list)).toBe(1);
  });

  it("无标签 / 不在名册 → 0（中性配色）", () => {
    expect(speakerSlot(null, roster("self"))).toBe(0);
    expect(speakerSlot("ghost", roster("self"))).toBe(0);
  });
});
