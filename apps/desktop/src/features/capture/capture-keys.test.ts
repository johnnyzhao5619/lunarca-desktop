/**
 * 捕获键盘判据（PRD-06-058 B / B″）。
 *
 * 它的全部价值在于：中文用户按回车上屏时不落一篇半截笔记。三种运行时给出的事实各不
 * 相同（表见 `lib/ime-composition.ts`），2026-09-09 真机栽的是 WKWebView 那一行。
 */

import { describe, expect, it } from "vitest";
import {
  COMPOSITION_COMMIT_WINDOW_MS,
  NO_COMPOSITION,
  type CompositionState,
} from "../../lib/ime-composition";
import { classifyCaptureKey, type CaptureKey } from "./capture-keys";

function key(overrides: Partial<CaptureKey> = {}): CaptureKey {
  return {
    key: "Enter",
    shiftKey: false,
    metaKey: false,
    ctrlKey: false,
    timeStamp: 1_000,
    ...overrides,
  };
}

describe("普通按键（没有输入法参与）", () => {
  it("回车 = 存下；⌘/Ctrl+回车 = 存下并打开；Shift+回车 = 换行", () => {
    expect(classifyCaptureKey(key())).toBe("save");
    expect(classifyCaptureKey(key({ metaKey: true }))).toBe("saveAndOpen");
    expect(classifyCaptureKey(key({ ctrlKey: true }))).toBe("saveAndOpen");
    expect(classifyCaptureKey(key({ shiftKey: true }))).toBe("newline");
  });

  it("非回车一律不管", () => {
    expect(classifyCaptureKey(key({ key: "a" }))).toBe("ignore");
    expect(classifyCaptureKey(key({ key: "Escape" }))).toBe("ignore");
  });

  it("Shift+回车与「让给输入法」是两个结论，对调用方却是同一个要求：别 preventDefault", () => {
    // 合并成一个结论会丢掉语义：换行是原生行为，上屏是别人的键。
    expect(classifyCaptureKey(key({ shiftKey: true }))).not.toBe("save");
  });
});

describe("输入法上屏的那一下，三种运行时三种事实", () => {
  it("Chromium / Gecko：keydown 早于 compositionend，`isComposing` 为真", () => {
    expect(classifyCaptureKey(key({ isComposing: true }))).toBe("ignore");
  });

  it("Windows 部分输入法：`isComposing` 为假，只剩 keyCode 229 这一条线索", () => {
    expect(classifyCaptureKey(key({ isComposing: false, keyCode: 229 }))).toBe("ignore");
  });

  it("WKWebView：合成先结束，那一下 keydown 与普通回车毫无区别", () => {
    // 真机 2026-09-09 的原样复现：isComposing 假、keyCode 13、无修饰键。
    const composition: CompositionState = { active: false, endedAt: 1_000 };
    const commit = key({ isComposing: false, keyCode: 13, timeStamp: 1_000 });
    expect(classifyCaptureKey(commit, composition)).toBe("ignore");
  });

  it("组字进行中（还没上屏）同样不提交", () => {
    const composition: CompositionState = { active: true, endedAt: Number.NEGATIVE_INFINITY };
    expect(classifyCaptureKey(key({ isComposing: false }), composition)).toBe("ignore");
  });
});

describe("反向条款：时间窗只认「同一次按键」，不吞用户真正的回车", () => {
  it("上屏之后再按一次回车 = 存下那串字（用户看着框里的内容按的）", () => {
    const composition: CompositionState = { active: false, endedAt: 1_000 };
    // 人类再按一次回车的最短间隔在 150ms 量级，远在窗外。
    expect(classifyCaptureKey(key({ timeStamp: 1_200 }), composition)).toBe("save");
  });

  it("窗口边界：恰好落在窗上算同一次，超出一毫秒就是新的一次", () => {
    const composition: CompositionState = { active: false, endedAt: 1_000 };
    const at = (t: number) => classifyCaptureKey(key({ timeStamp: t }), composition);
    expect(at(1_000 + COMPOSITION_COMMIT_WINDOW_MS)).toBe("ignore");
    expect(at(1_000 + COMPOSITION_COMMIT_WINDOW_MS + 1)).toBe("save");
  });

  it("从未组过字时，时间窗判定永远不会误命中", () => {
    // `endedAt` 取负无穷正是为了这一条：取 0 时，timeStamp 很小的早期按键会被误吞。
    expect(classifyCaptureKey(key({ timeStamp: 0 }), NO_COMPOSITION)).toBe("save");
    expect(classifyCaptureKey(key({ timeStamp: 5 }))).toBe("save");
  });

  it("⌘回车同样受合成边界保护，但窗外照常「存下并打开」", () => {
    const composition: CompositionState = { active: false, endedAt: 1_000 };
    expect(classifyCaptureKey(key({ metaKey: true, timeStamp: 1_000 }), composition)).toBe(
      "ignore",
    );
    expect(classifyCaptureKey(key({ metaKey: true, timeStamp: 2_000 }), composition)).toBe(
      "saveAndOpen",
    );
  });
});
