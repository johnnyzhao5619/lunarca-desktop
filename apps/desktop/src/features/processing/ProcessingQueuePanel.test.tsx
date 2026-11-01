// @vitest-environment happy-dom
/**
 * 处理队列面板的结构守护（PRD-04-055 三段修订，2026-09-07）。
 *
 * 钉住四件真机上出过的事：
 * ① 失败原因**完整可读**——渲染的是人话、`title` 是同一句话（此前 title 是诊断码，正文被
 *    45% 宽 + nowrap 截成「任务未能完成。请检查所…」）；
 * ② 失败项归「未成功」段，不再顶着「已完成」的标题；「清除已完成」只在有成功项时出现；
 * ③ 已完成项用一枚真按钮「打开」跳转，整行 `li` 不挂点击（键盘可达）；
 * ④ 排队项显示位次，上移 / 下移是两枚不同字形（不靠 CSS 旋转区分）。
 */
import { createElement, type ReactNode } from "react";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createInstance, type i18n as I18nInstance } from "i18next";
import { I18nextProvider } from "react-i18next";

import workspace from "../../i18n/locales/zh-CN/workspace.json";
import transcribe from "../../i18n/locales/zh-CN/transcribe.json";
import common from "../../i18n/locales/zh-CN/common.json";
import type { ProcStatus, ProcTaskDto } from "../../ipc/bindings";
import { ConfirmHost } from "../../components/ConfirmHost";
import { ProcessingQueuePanel } from "./ProcessingQueuePanel";

const i18n: I18nInstance = createInstance();
void i18n.init({
  lng: "zh-CN",
  fallbackLng: "zh-CN",
  defaultNS: "workspace",
  resources: { "zh-CN": { workspace, transcribe, common } },
  interpolation: { escapeValue: false },
});

function task(id: string, status: ProcStatus, label = `note-${id}`): ProcTaskDto {
  return {
    id,
    kind: "merge",
    label,
    note_rel: null,
    status,
    executed_by: null,
    queue: null,
    restored: false,
  };
}

const noop = () => {};
const visit = {
  markInteracted: noop,
  interactionProps: {},
} as unknown as React.ComponentProps<typeof ProcessingQueuePanel>["visit"];

function mount(items: ProcTaskDto[], overrides: Partial<React.ComponentProps<typeof ProcessingQueuePanel>> = {}) {
  const wrap = (children: ReactNode) =>
    createElement(I18nextProvider, { i18n }, createElement(ConfirmHost, null, children));
  return render(
    wrap(
      createElement(ProcessingQueuePanel, {
        items,
        onReorder: noop,
        onRemove: noop,
        onCancel: noop,
        onRetry: noop,
        onClearDone: noop,
        onClearUnsuccessful: noop,
        onSelectNote: noop,
        onClose: noop,
        visit,
        ...overrides,
      }),
    ),
  );
}

const failed: ProcStatus = { state: "failed", data: { code: "DISTILL_JOB: QUOTA_EXHAUSTED", retryable: true, retry_charges_again: false } };
const done: ProcStatus = { state: "done", data: { note_rel: "Notes/a.md", awaiting_review: 0 } };

describe("处理队列面板（三段）", () => {
  // 无 globals 时 RTL 不自动清理：不清的话下一个用例会查到上一个用例的面板。
  afterEach(cleanup);

  it("失败原因是完整人话，title 是同一句话，诊断码另置小字", () => {
    mount([task("1", failed)]);
    const reason = document.querySelector(".luna-proc-queue__reason") as HTMLElement;
    expect(reason).not.toBeNull();
    const text = reason.textContent ?? "";
    expect(text).toContain("额度");
    expect(reason.getAttribute("title")).not.toContain("QUOTA_EXHAUSTED");
    expect(text).toContain(reason.getAttribute("title") ?? "∅");
    expect(reason.querySelector("code")?.textContent).toBe("DISTILL_JOB: QUOTA_EXHAUSTED");
  });

  it("结果取不回的失败：重试按钮自己说出会再次计费（判据来自 Rust）", () => {
    const charged: ProcStatus = {
      state: "failed",
      data: { code: "TRANSCRIBE_JOB: RESULT_UNAVAILABLE", retryable: true, retry_charges_again: true },
    };
    const { container } = mount([task("1", charged), task("2", failed)]);
    const labels = Array.from(container.querySelectorAll(".luna-proc-queue__item-actions button")).map(
      (el) => el.textContent ?? "",
    );
    expect(labels.some((label) => label.includes("会再次计费"))).toBe(true);
    expect(labels.filter((label) => label.includes("会再次计费"))).toHaveLength(1);
  });

  it("失败与取消归「未成功」段，成功归「已完成」段；两枚清除各自只在本段非空时出现", () => {
    const { container } = mount([task("1", failed), task("2", { state: "cancelled" }), task("3", done)]);
    const titles = Array.from(container.querySelectorAll(".luna-proc-queue__section-title")).map(
      (el) => el.textContent,
    );
    expect(titles).toEqual(["未成功", "已完成"]);
    // 两枚「清除」收进页头「⋯」（ADR-137 H8）；「清除未成功」是危险项。关闭走 IconButton 原语。
    expect(screen.getByRole("button", { name: "关闭" }).className).toContain("luna-icon-btn");
    fireEvent.click(screen.getByRole("button", { name: "整理队列" }));
    expect(screen.getByRole("menuitem", { name: "清除已完成" })).toBeTruthy();
    expect(screen.getByRole("menuitem", { name: "清除未成功" }).getAttribute("data-tone")).toBe("danger");
    cleanup();

    const second = mount([task("4", failed)]);
    fireEvent.click(within(second.container).getByRole("button", { name: "整理队列" }));
    expect(within(second.container).queryByRole("menuitem", { name: "清除已完成" })).toBeNull();
    expect(within(second.container).getByRole("menuitem", { name: "清除未成功" })).toBeTruthy();
  });

  it("已完成项用真按钮「打开」跳转，整行不挂点击", () => {
    const onSelectNote = vi.fn();
    mount([task("1", done)], { onSelectNote });
    const row = document.querySelector('.luna-proc-queue__item[data-state="done"]') as HTMLElement;
    expect(row.getAttribute("data-clickable")).toBeNull();
    const open = within(row).getByRole("button", { name: "打开" });
    fireEvent.click(open);
    expect(onSelectNote).toHaveBeenCalledWith("Notes/a.md");
  });

  it("上次会话回填的失败项：标「上次会话」、原因可读、没有重试、有移除（PRD-04-062）", () => {
    mount([{ ...task("9", failed), restored: true }]);
    expect(screen.getByText("上次会话")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "重试" })).toBeNull();
    expect(screen.getByRole("button", { name: "移除" })).toBeTruthy();
    expect(document.querySelector(".luna-proc-queue__reason")?.textContent).toContain("额度");
  });

  it("排队项显示位次；调序与移除在行菜单里，上移 / 下移是两枚不同字形，不靠旋转区分", () => {
    mount([task("1", { state: "queued" }), task("2", { state: "queued" })]);
    expect(screen.getByText("第 2 位")).toBeTruthy();
    // 每行只露一个入口（ADR-137 H8）：三枚图标钮收进行菜单。
    fireEvent.click(screen.getAllByRole("button", { name: "调整顺序" })[1]);
    const up = screen.getByRole("menuitem", { name: "上移" });
    const downBtn = screen.getByRole("menuitem", { name: "下移" });
    expect(screen.getByRole("menuitem", { name: "移除" })).toBeTruthy();
    expect(up.querySelector("svg")?.innerHTML).not.toBe(downBtn.querySelector("svg")?.innerHTML);
    const css = readFileSync(resolve(__dirname, "processing.css"), "utf8");
    expect(css).not.toContain("rotate(180deg)");
    // 状态位不再截断长句：原因走独立一行，状态位只放短标签。
    expect(css).not.toMatch(/__item-status[\s\S]*?max-inline-size: 45%/);
  });

  it("入口用托盘字形，不再借用「重试」的旋转箭头", () => {
    const host = readFileSync(resolve(__dirname, "./ProcessingQueueEntry.tsx"), "utf8");
    expect(host).not.toContain("ArrowClockwiseIcon");
    expect(host).toContain("<QueueIcon");
  });
});
