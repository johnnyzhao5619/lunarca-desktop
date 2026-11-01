// @vitest-environment happy-dom
/**
 * 页外结果提示（ADR-167 ①）：动作常常同时换页（删掉正看着的项目 → 回看板），这一句要由换上来
 * 的那一页接住，而不是说给即将卸下的旧页就丢了；过时的不再翻出来。
 */
import { createElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";

import { withI18n } from "../../i18n/i18n-test-fixture";
import { PageNotice, postPageNotice, usePageNotice } from "./CreatorPage";

function Page() {
  const { notice, clear } = usePageNotice();
  return createElement(PageNotice, { notice, onDismiss: clear });
}

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("页外结果提示", () => {
  it("说给正显示的那一页；紧接着换上来的那一页也接住", () => {
    const first = render(withI18n(createElement(Page)));
    act(() => postPageNotice({ text: "已删除项目「丙」。", tone: "info" }));
    expect(screen.getByRole("status").textContent).toContain("已删除项目「丙」。");
    first.unmount();
    render(withI18n(createElement(Page)));
    expect(screen.getByRole("status").textContent).toContain("已删除项目「丙」。");
  });

  it("隔了一会儿才换上来的页不再翻出过时的那一句，也只翻一次", () => {
    vi.useFakeTimers();
    postPageNotice({ text: "旧消息", tone: "info" });
    vi.advanceTimersByTime(5000);
    const late = render(withI18n(createElement(Page)));
    expect(screen.queryByRole("status")).toBeNull();
    late.unmount();
    postPageNotice({ text: "新消息", tone: "info" });
    const next = render(withI18n(createElement(Page)));
    expect(screen.getByRole("status").textContent).toContain("新消息");
    next.unmount();
    render(withI18n(createElement(Page)));
    expect(screen.queryByRole("status")).toBeNull();
  });
});
