/**
 * 弹窗翻译输入框的 Enter 分类（PRD-01-005）。
 *
 * 这份判据的全部价值在于**中文用户按回车上屏候选时不把半句话拿去翻译**，而那件事在三种
 * 运行时给出的事实各不相同（表见 `lib/ime-composition.ts`）。最险的是 WKWebView 那一行：
 * 合成在 keydown **之前**就结束了，那一下身上一点"我属于输入法"的痕迹都没有。
 */

import { describe, expect, it } from "vitest";
import {
  COMPOSITION_COMMIT_WINDOW_MS,
  NO_COMPOSITION,
  type CompositionState,
} from "../../lib/ime-composition";
import { classifyPopupInputKey, type PopupInputKey } from "./popup-keys";

function key(overrides: Partial<PopupInputKey> = {}): PopupInputKey {
  return {
    key: "Enter",
    shiftKey: false,
    altKey: false,
    metaKey: false,
    ctrlKey: false,
    timeStamp: 1_000,
    ...overrides,
  };
}

describe("弹窗翻译输入框的 Enter 分类（PRD-01-005）", () => {
  it("Enter / Cmd+Enter / Ctrl+Enter 都是生成", () => {
    expect(classifyPopupInputKey(key())).toBe("generate");
    expect(classifyPopupInputKey(key({ metaKey: true }))).toBe("generate");
    expect(classifyPopupInputKey(key({ ctrlKey: true }))).toBe("generate");
  });

  it("Shift+Enter / Alt+Enter 是换行，交给 textarea", () => {
    expect(classifyPopupInputKey(key({ shiftKey: true }))).toBe("newline");
    expect(classifyPopupInputKey(key({ altKey: true }))).toBe("newline");
  });

  it("其它键一律不管", () => {
    expect(classifyPopupInputKey(key({ key: "a" }))).toBe("ignore");
    expect(classifyPopupInputKey(key({ key: "Escape" }))).toBe("ignore");
  });
});

describe("输入法上屏的那一下，三种运行时三种事实（绝不生成）", () => {
  it("Chromium / Gecko：keydown 早于 compositionend，`isComposing` 为真", () => {
    expect(classifyPopupInputKey(key({ isComposing: true }))).toBe("ignore");
  });

  it("Windows 部分输入法：`isComposing` 为假，只剩 keyCode 229 这一条线索", () => {
    expect(classifyPopupInputKey(key({ isComposing: false, keyCode: 229 }))).toBe("ignore");
  });

  it("WKWebView：合成先结束，那一下 keydown 与普通回车**毫无区别**", () => {
    // 弹窗翻译此前正是在这一条上失守：单看事件本身它就是一次普通回车，于是一句还没打完
    // 的话被拿去翻译。只有组件记下的合成边界能把它认出来。
    const composition: CompositionState = { active: false, endedAt: 1_000 };
    const commit = key({ isComposing: false, keyCode: 13, timeStamp: 1_000 });
    expect(classifyPopupInputKey(commit, composition)).toBe("ignore");
  });

  it("组字进行中（还没上屏）同样不生成", () => {
    const composition: CompositionState = { active: true, endedAt: Number.NEGATIVE_INFINITY };
    expect(classifyPopupInputKey(key({ isComposing: false }), composition)).toBe("ignore");
  });
});

describe("反向条款：时间窗只认「同一次按键」，不吞用户真正的回车", () => {
  it("上屏之后再按一次回车 = 照常生成（用户看着框里的内容按的）", () => {
    const composition: CompositionState = { active: false, endedAt: 1_000 };
    // 人类"上屏之后再按一次回车"的最短间隔在 150ms 量级，远在窗外。
    expect(classifyPopupInputKey(key({ timeStamp: 1_200 }), composition)).toBe("generate");
  });

  it("窗口边界：恰好落在窗上算同一次，超出一毫秒就是新的一次", () => {
    const composition: CompositionState = { active: false, endedAt: 1_000 };
    const at = (t: number) => classifyPopupInputKey(key({ timeStamp: t }), composition);
    expect(at(1_000 + COMPOSITION_COMMIT_WINDOW_MS)).toBe("ignore");
    expect(at(1_000 + COMPOSITION_COMMIT_WINDOW_MS + 1)).toBe("generate");
  });

  it("从未组过字时，时间窗判定永远不会误命中", () => {
    // `endedAt` 取负无穷正是为了这一条：取 0 的话，`timeStamp` 很小的早期按键会被误吞。
    expect(classifyPopupInputKey(key({ timeStamp: 0 }), NO_COMPOSITION)).toBe("generate");
    expect(classifyPopupInputKey(key({ timeStamp: 5 }))).toBe("generate");
  });

  it("Shift+Enter 在窗内是「让给输入法」，窗外照常换行——两者都不得被 preventDefault", () => {
    const composition: CompositionState = { active: false, endedAt: 1_000 };
    expect(classifyPopupInputKey(key({ shiftKey: true, timeStamp: 1_000 }), composition)).toBe(
      "ignore",
    );
    expect(classifyPopupInputKey(key({ shiftKey: true, timeStamp: 2_000 }), composition)).toBe(
      "newline",
    );
  });
});
