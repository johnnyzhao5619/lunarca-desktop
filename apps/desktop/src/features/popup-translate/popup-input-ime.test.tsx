// @vitest-environment happy-dom
/**
 * 弹窗翻译输入框的**输入法接线**守护（PRD-01-005）。
 *
 * 判据本身归 `popup-keys.test.ts`（纯函数，三种运行时逐条）。这里要证明的是另一半——组件
 * 真的把合成边界记下来了、真的从 `nativeEvent` 取 `isComposing`、且**没有把让给输入法的
 * 那一下 `preventDefault` 掉**。这三件事在纯函数测试里一条都看不见：判据可以完美无缺，而
 * 组件根本没接上去，或者接上了却顺手把事件吃了——后者的表现是候选词永远上不了屏，比
 * "被误翻译"更难查。速记那一轮（2026-09-09）正是这半边先漏出去的。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

// 生成走二进制 Channel（ADR-011），构造它要 Tauri 运行时内部对象；与本文件要证明的事无关。
vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null;
  },
}));

// HUD 外壳借 Tauri 的窗口 API 自动增高；与本文件要证明的事无关，替掉即可。
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    setSize: () => Promise.resolve(),
    startDragging: () => Promise.resolve(),
  }),
  LogicalSize: class {
    constructor(
      public width: number,
      public height: number,
    ) {}
  },
}));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { COMPOSITION_COMMIT_WINDOW_MS } from "../../lib/ime-composition";
import { PopupTranslateView } from "./PopupTranslateView";

const PRESETS = [{ id: "to_en", name: "英文", group: "target_lang" }];

/** 输入模式、无选区、无草稿——用户刚用热键唤起一个空输入框。 */
const EMPTY_INPUT_SESSION = {
  status: "ok",
  data: {
    mode: "input",
    has_source: false,
    source_text: null,
    save_to_translate_space: false,
    draft: null,
  },
};

/**
 * 「有没有开始生成」的可观察判据：生成的第一步是把输入升格为会话源文本
 * （`ensureInputSource` → `popup_set_input`）。它先于任何流式往返发生，故不必让 Channel 真跑。
 */
function generated() {
  return commandMock("popupSetInput").mock.calls.length > 0;
}

async function mount() {
  const view = render(withI18n(<PopupTranslateView presets={PRESETS} />));
  const input = await waitFor(() => {
    const el = view.container.querySelector("textarea");
    if (!el) throw new Error("输入框还没渲染出来");
    return el;
  });
  return { view, input };
}

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("popupSessionState", EMPTY_INPUT_SESSION);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("弹窗翻译输入框 × 中文输入法（PRD-01-005）", () => {
  it("WKWebView 顺序：`compositionend` 先于 `keydown`，那一下不得触发生成", async () => {
    // 真机上"上屏"那一下身上 `isComposing` 是假、keyCode 是 13——单看事件它就是一次普通
    // 回车，于是一句还没打完的话被拿去翻译。只有组件记下的合成边界能把它认出来。
    const { input } = await mount();
    fireEvent.change(input, { target: { value: "今天天气" } });
    fireEvent.compositionEnd(input, { data: "气" });
    fireEvent.keyDown(input, { key: "Enter", isComposing: false, keyCode: 13 });
    await Promise.resolve();
    expect(generated(), "上屏那一下被当成了「生成」").toBe(false);
  });

  it("让给输入法的那一下必须原样放行——吃掉它，候选词就永远上不了屏", async () => {
    const { input } = await mount();
    fireEvent.change(input, { target: { value: "今天天气" } });
    fireEvent.compositionEnd(input, { data: "气" });
    const notPrevented = fireEvent.keyDown(input, { key: "Enter", isComposing: false });
    expect(notPrevented, "事件被 preventDefault 了，输入法拿不到这一下").toBe(true);
  });

  it("组字进行中（`compositionstart` 之后还没上屏）同样不生成", async () => {
    const { input } = await mount();
    fireEvent.change(input, { target: { value: "jintian" } });
    fireEvent.compositionStart(input, { data: "j" });
    fireEvent.keyDown(input, { key: "Enter", isComposing: false });
    await Promise.resolve();
    expect(generated()).toBe(false);
  });

  it("`isComposing` 必须从 `nativeEvent` 取——React 合成事件根本不带这个字段", async () => {
    // 没有合成边界、只有 `isComposing` 的运行时（Chromium / Gecko）全靠这一条。取错地方
    // 时它恒为 `undefined`，本用例会红而其它用例照绿。
    const { input } = await mount();
    fireEvent.change(input, { target: { value: "今天天气" } });
    fireEvent.keyDown(input, { key: "Enter", isComposing: true });
    await Promise.resolve();
    expect(generated()).toBe(false);
  });

  it("反向条款：上屏之后再按一次回车 = 照常生成", async () => {
    // 合成边界只用来认「同一次按键」。用户看着框里已上屏的字又按了一次回车，那就是他真的
    // 要翻译它——把这一下也吞掉，中文用户就再也翻不了东西了。
    //
    // 这里必须**真的等**而不能用假时钟：判据读的是事件自己的 `timeStamp`（单调时钟），
    // `vi.advanceTimersByTime` 推不动它——假时钟下这条用例只会证明"时间没走时不生成"。
    const { input } = await mount();
    fireEvent.change(input, { target: { value: "今天天气" } });
    fireEvent.compositionEnd(input, { data: "气" });
    await new Promise((resolve) => setTimeout(resolve, COMPOSITION_COMMIT_WINDOW_MS * 2));
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(commandMock("popupSetInput")).toHaveBeenCalledWith("今天天气"));
  });

  it("没有输入法参与时，回车照常生成（没把普通路径连坐）", async () => {
    const { input } = await mount();
    fireEvent.change(input, { target: { value: "hello" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(commandMock("popupSetInput")).toHaveBeenCalledWith("hello"));
  });
});
