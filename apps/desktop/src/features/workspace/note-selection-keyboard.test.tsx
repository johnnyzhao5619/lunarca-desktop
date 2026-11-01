// @vitest-environment happy-dom
/**
 * 选择原语的**键盘行为**守护（PRD-06-059 H）。
 *
 * 与同目录的源码守护分工不同：那一条钉"接线有没有从原语上取"，这一条真渲染一份行清单、
 * 真按键，钉的是**按下去到底发生了什么**——↑↓ 挪选择、Shift+↑↓ 连选、Esc 收回多选、
 * Delete/Backspace 走删除、方向键不让页面滚。
 *
 * **平台那一半测不到，也别假装测到了**：真机上这些键全都没反应，根因是 macOS 的
 * WKWebView 点击按钮不给焦点、且 mousedown 默认行为把焦点收回 body（2026-09-02）。
 * happy-dom 不复刻那个怪癖，故那一半只能靠 `rowProps` 里的 `preventDefault + focus`
 * 与真机验证；这里保证的是"只要焦点在这棵子树里，按键的语义就是对的"。
 */

import { createElement, useEffect, useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useNoteSelection } from "./use-note-selection";

const ROWS = ["a.md", "b.md", "c.md", "d.md"];

function Harness({ onDelete }: { onDelete?: () => void }) {
  const [current, setCurrent] = useState<string | null>("a.md");
  const selection = useNoteSelection({ current, visible: ROWS, onDelete });
  return createElement(
    "ul",
    { ...selection.containerProps, "data-testid": "rows" },
    ROWS.map((path) =>
      createElement(
        "li",
        { key: path, role: "none" },
        createElement("button", {
          type: "button",
          // `aria-selected` 由 `rowProps` 贴（视图不各写一遍），这里跟真实调用点同形。
          ...selection.rowProps(path),
          "aria-current": current === path || undefined,
          onClick: (event: React.MouseEvent) => {
            if (selection.click(path, event)) {
              setCurrent(path);
            }
          },
          children: path,
        }),
      ),
    ),
  );
}

/**
 * 与 `Harness` 同形，只是**可见行像真实清单那样后一帧才报上来**——`FolderedItems` /
 * `FilteredNoteList` / `ChatHistoryList` 都是在自己的 effect 里回流可见行的。
 */
function LateRowsHarness() {
  const [current, setCurrent] = useState<string | null>("a.md");
  const [visible, setVisible] = useState<readonly string[]>([]);
  useEffect(() => {
    setVisible(ROWS);
  }, []);
  const selection = useNoteSelection({ current, visible });
  return createElement(
    "ul",
    { ...selection.containerProps, "data-testid": "rows" },
    ROWS.map((path) =>
      createElement(
        "li",
        { key: path, role: "none" },
        createElement("button", {
          type: "button",
          ...selection.rowProps(path),
          "aria-current": current === path || undefined,
          onClick: (event: React.MouseEvent) => {
            if (selection.click(path, event)) {
              setCurrent(path);
            }
          },
          children: path,
        }),
      ),
    ),
  );
}

/** 此刻被选中的那几行（按屏幕顺序）。 */
function selected(): string[] {
  return screen
    .getAllByRole("button")
    .filter((row) => row.getAttribute("aria-selected") === "true")
    .map((row) => row.textContent ?? "");
}

function keyOnRows(key: string, init: Partial<KeyboardEventInit> = {}) {
  return fireEvent.keyDown(screen.getByTestId("rows"), { key, ...init });
}

describe("多选的键盘契约", () => {
  it("↑↓ 把选择挪到相邻行，并**吃掉按键**（否则页面跟着滚）", () => {
    cleanup();
    render(createElement(Harness));
    expect(selected()).toEqual(["a.md"]);
    // `fireEvent` 返回 false = 有人调了 preventDefault：那正是"页面不该跟着滚"。
    expect(keyOnRows("ArrowDown"), "方向键没被吃掉，浏览器会拿去滚动页面").toBe(false);
    expect(selected()).toEqual(["b.md"]);
    keyOnRows("ArrowDown");
    expect(selected()).toEqual(["c.md"]);
    keyOnRows("ArrowUp");
    expect(selected()).toEqual(["b.md"]);
  });

  it("Shift+↑↓ 从锚点连选，Esc 收回多选", () => {
    cleanup();
    render(createElement(Harness));
    keyOnRows("ArrowDown"); // 锚点落在 b
    keyOnRows("ArrowDown", { shiftKey: true });
    expect(selected()).toEqual(["b.md", "c.md"]);
    keyOnRows("ArrowDown", { shiftKey: true });
    expect(selected()).toEqual(["b.md", "c.md", "d.md"]);
    keyOnRows("Escape");
    // 收回到"只剩当前打开的那一条"（这份夹具里当前仍是 a）。
    expect(selected()).toEqual(["a.md"]);
  });

  it("Home / End 跳到两端", () => {
    cleanup();
    render(createElement(Harness));
    keyOnRows("End");
    expect(selected()).toEqual(["d.md"]);
    keyOnRows("Home");
    expect(selected()).toEqual(["a.md"]);
  });

  it("Delete 与 Backspace 都触发删除——Mac 上标着 delete 的那个键叫 Backspace", () => {
    cleanup();
    const onDelete = vi.fn();
    render(createElement(Harness, { onDelete }));
    expect(keyOnRows("Backspace"), "按键没被吃掉，浏览器会拿去后退/滚动").toBe(false);
    keyOnRows("Delete");
    expect(onDelete).toHaveBeenCalledTimes(2);
  });

  it("可见行是**上报**上来的：第一帧为空也不能把当前那条剪掉", () => {
    // 真机 2026-09-09「速记 / AI 对话无法多选」的根因。清单的可见行由子组件在自己的
    // effect 里回流，所以父组件第一帧拿到的必然是空数组；照直按它剪枝，初始那条单选当场
    // 消失，而它只在 `current` 变化时才重建——于是第一次 Cmd 加选只亮一行，Shift 连选
    // 连锚点都没有，直接退化成单选。
    cleanup();
    render(createElement(LateRowsHarness));
    // 行报上来之后，当前那条仍在选择集里（`aria-current` 与 `aria-selected` 同时在）。
    expect(selected()).toEqual(["a.md"]);
    fireEvent.click(screen.getByText("c.md"), { shiftKey: true });
    expect(selected()).toEqual(["a.md", "b.md", "c.md"]);
  });

  it("没有删除通道的清单不吞 Delete（不假装自己处理了）", () => {
    cleanup();
    render(createElement(Harness));
    expect(keyOnRows("Delete")).toBe(true);
  });
});
