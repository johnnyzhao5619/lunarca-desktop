// @vitest-environment happy-dom
/**
 * 统一行差异视图（PRD-06-041、PRD-21-030）：只渲染 Rust 给的差异——每行带新旧行号、类别与 +/− 记号，
 * 正文原样显示（不当 Markdown 解析、不当 HTML 插入）。
 */
import { createElement } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render } from "@testing-library/react";
import { DiffView, type LineDiff } from "./DiffView";

const DIFF: LineDiff = {
  lines: [
    { kind: "equal", old_line: 1, new_line: 1, text: "a" },
    { kind: "removed", old_line: 2, new_line: null, text: "b" },
    { kind: "added", old_line: null, new_line: 2, text: "<b>B</b> **粗**" },
    { kind: "equal", old_line: 3, new_line: 3, text: "c" },
  ],
  added: 1,
  removed: 1,
  degraded: false,
};

afterEach(cleanup);

describe("DiffView", () => {
  it("逐行画出类别、行号与记号", () => {
    const { container } = render(createElement(DiffView, { diff: DIFF }));
    const lines = [...container.querySelectorAll(".luna-diff__line")];
    expect(lines.map((line) => line.getAttribute("data-kind"))).toEqual(["equal", "removed", "added", "equal"]);
    const cells = (index: number) => [...lines[index].querySelectorAll("span")].map((span) => span.textContent);
    expect(cells(1)).toEqual(["2", "", "−", "b"]);
    expect(cells(2)).toEqual(["", "2", "+", "<b>B</b> **粗**"]);
    expect(cells(0)[2]).toBe(" ");
  });

  it("正文是纯文本：不插入 HTML 元素", () => {
    const { container } = render(createElement(DiffView, { diff: DIFF }));
    expect(container.querySelector(".luna-diff b")).toBeNull();
  });

  it("两版相同（没有行）时只留一个空容器，措辞归调用方", () => {
    const { container } = render(createElement(DiffView, { diff: { lines: [], added: 0, removed: 0, degraded: false } }));
    expect(container.querySelector(".luna-diff")?.childElementCount).toBe(0);
  });
});
