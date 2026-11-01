// @vitest-environment happy-dom
/**
 * 「起个名字」输入卡的**渲染行为**守护（PRD-06-006）。
 *
 * 这里钉的是一个真实发生过的缺陷：打开输入框时的"聚焦并全选"如果依赖了**草稿**，
 * 每敲一个字符就会把整串重新选上，下一个字符直接覆盖掉全部内容——用户几乎完成不了
 * 一次重命名（2026-09-02 真机上报，原形在文件树的行内改名里）。
 *
 * 故这条用例不看源码写法，看**打完三个字符之后输入框里是什么**：只要"全选"被挂回每次
 * 渲染，字符就会互相覆盖，这里立刻变红。
 */

import { createElement, useEffect } from "react";
import { describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { createInstance, type i18n as I18nInstance } from "i18next";
import { I18nextProvider } from "react-i18next";
import { PromptHost, usePrompt, type PromptFn } from "./PromptHost";
import zhCommon from "../i18n/locales/zh-CN/common.json";

const i18n: I18nInstance = createInstance();
await i18n.init({
  lng: "zh-CN",
  fallbackLng: "zh-CN",
  defaultNS: "common",
  resources: { "zh-CN": { common: zhCommon } },
  interpolation: { escapeValue: false },
});

/** 一开张就发起提问的探针（调用点在真实使用里也都是命令式的）。 */
function Ask({ onSettled }: { onSettled: (value: string | null) => void }) {
  const prompt: PromptFn = usePrompt();
  useEffect(() => {
    void prompt({ title: "重命名「会议」", label: "新名称", value: "会议" }).then(onSettled);
  }, [prompt, onSettled]);
  return null;
}

function mount() {
  const onSettled = vi.fn();
  render(
    createElement(
      I18nextProvider,
      { i18n },
      createElement(PromptHost, null, createElement(Ask, { onSettled })),
    ),
  );
  return { onSettled };
}

/**
 * 像用户那样敲一个字：**替换掉此刻的选区**，把光标留在插入点之后，然后让 React 把这一
 * 轮渲染与副作用跑完。
 *
 * 顺序照抄浏览器：值与光标先落定，React 的重渲染与 effect 随后。少了最后那一步
 * flush，"每次渲染都重新全选"这个缺陷会被测试自己的时序掩盖过去——那样这条用例就只是
 * 看着像在守护。
 */
function type(input: HTMLInputElement, text: string) {
  const start = input.selectionStart ?? input.value.length;
  const end = input.selectionEnd ?? input.value.length;
  const next = input.value.slice(0, start) + text + input.value.slice(end);
  const setter = Object.getOwnPropertyDescriptor(
    window.HTMLInputElement.prototype,
    "value",
  )?.set;
  setter?.call(input, next);
  input.setSelectionRange(start + text.length, start + text.length);
  act(() => {
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

describe("重命名输入卡", () => {
  it("打开时全选一次，之后逐字累加——不是每敲一个字符就重新全选", async () => {
    cleanup();
    mount();
    const input = (await screen.findByLabelText("新名称")) as HTMLInputElement;
    // 开张即全选：用户直接打字就是覆盖旧名字（Finder / VS Code 同行为）。
    await waitFor(() => {
      expect(input.selectionStart).toBe(0);
      expect(input.selectionEnd).toBe("会议".length);
    });
    type(input, "周");
    type(input, "会");
    type(input, "纪要");
    expect(input.value).toBe("周会纪要");
  });

  it("空输入等同取消（不落一次改名成空名的命令）", async () => {
    cleanup();
    const { onSettled } = mount();
    const input = (await screen.findByLabelText("新名称")) as HTMLInputElement;
    type(input, "   ");
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await waitFor(() => expect(onSettled).toHaveBeenCalledWith(null));
  });
});
