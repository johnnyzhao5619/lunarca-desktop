// @vitest-environment happy-dom
/// <reference types="vite/client" />
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { isImeKeyEvent } from "./ime-key-event";
import { COMPOSITION_COMMIT_WINDOW_MS } from "./ime-composition";

function at<T extends Event>(event: T, timeStamp: number): T {
  Object.defineProperty(event, "timeStamp", { value: timeStamp });
  return event;
}

function keydown(target: EventTarget, timeStamp: number, init: KeyboardEventInit = {}): KeyboardEvent {
  const event = at(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, ...init }), timeStamp);
  target.dispatchEvent(event);
  return event;
}

function compose(target: EventTarget, type: "compositionstart" | "compositionend", timeStamp: number) {
  target.dispatchEvent(at(new CompositionEvent(type, { bubbles: true }), timeStamp));
}

let input: HTMLInputElement;
let other: HTMLInputElement;

beforeEach(() => {
  input = document.createElement("input");
  other = document.createElement("input");
  document.body.append(input, other);
  // 上一条用例留下的边界一律推到很久以前。
  compose(input, "compositionend", -1e9);
  input.focus();
});

afterEach(() => {
  input.remove();
  other.remove();
});

describe("isImeKeyEvent：文档级合成边界（PRD-07-040 ①）", () => {
  it("从未组字时，普通回车不属于输入法", () => {
    expect(isImeKeyEvent(keydown(input, 1_000))).toBe(false);
  });

  it("WKWebView 时序：compositionend 先到，紧随其后的回车属于输入法", () => {
    compose(input, "compositionstart", 900);
    compose(input, "compositionend", 1_000);
    expect(isImeKeyEvent(keydown(input, 1_000 + COMPOSITION_COMMIT_WINDOW_MS))).toBe(true);
  });

  it("时间窗外再按一次回车照常执行", () => {
    compose(input, "compositionstart", 900);
    compose(input, "compositionend", 1_000);
    expect(isImeKeyEvent(keydown(input, 1_000 + COMPOSITION_COMMIT_WINDOW_MS + 1))).toBe(false);
  });

  it("组字中（compositionstart 之后）的按键属于输入法，即便事件字段看不出来", () => {
    compose(input, "compositionstart", 900);
    expect(isImeKeyEvent(keydown(input, 5_000, { key: "Escape" }))).toBe(true);
    expect(isImeKeyEvent(keydown(input, 5_000, { key: "ArrowDown" }))).toBe(true);
  });

  it("发起合成的元素已失焦时不再算组字中——收不到 compositionend 也不会把全应用的回车永久判给输入法", () => {
    compose(input, "compositionstart", 900);
    other.focus();
    expect(isImeKeyEvent(keydown(other, 5_000))).toBe(false);
  });

  it("挂在 document 上的处理（对话框的 Esc）问同一个问题，得到同一个答案", () => {
    compose(input, "compositionstart", 900);
    compose(input, "compositionend", 1_000);
    let seen: boolean | null = null;
    const onKeyDown = (event: KeyboardEvent) => {
      seen = isImeKeyEvent(event);
    };
    document.addEventListener("keydown", onKeyDown);
    keydown(input, 1_010, { key: "Escape" });
    document.removeEventListener("keydown", onKeyDown);
    expect(seen).toBe(true);
  });

  it("React 合成事件形状：从 nativeEvent 取", () => {
    const native = keydown(input, 1_000, { isComposing: true } as KeyboardEventInit);
    expect(isImeKeyEvent({ nativeEvent: native })).toBe(true);
  });
});
