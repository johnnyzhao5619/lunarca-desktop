// @vitest-environment happy-dom
/**
 * 无边框窗口的拖动判定（真机 2026-09-09：便签上的「在 Lunarca 中打开」点了没反应）。
 *
 * 根因是判据只认 `HTMLElement`，而图标按钮里那个 `<svg>` 是 `SVGElement`——点在图标的
 * 笔画上就被判成"点在空白处"，`startDragging()` 接管指针，那一下的 `click` 根本不发生。
 * 表现是**同一颗按钮，点在图标上没反应、点在它周围的内距上正常**。
 */

import { describe, expect, it } from "vitest";
import { isInteractiveTarget } from "./use-window-drag";

function build(html: string): HTMLElement {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host;
}

describe("拖动把手上的落点判定", () => {
  it("点在图标的笔画上算点在按钮上（SVG 不是 HTMLElement）", () => {
    const host = build(
      '<button type="button"><svg><path d="M0 0"></path></svg></button>',
    );
    expect(isInteractiveTarget(host.querySelector("path"))).toBe(true);
    expect(isInteractiveTarget(host.querySelector("svg"))).toBe(true);
    expect(isInteractiveTarget(host.querySelector("button"))).toBe(true);
  });

  it("显式标了 data-no-drag 的整块都不拖", () => {
    const host = build('<div data-no-drag><span><i></i></span></div>');
    expect(isInteractiveTarget(host.querySelector("i"))).toBe(true);
  });

  it("空白处照常拖", () => {
    const host = build('<header><span>标题</span></header>');
    expect(isInteractiveTarget(host.querySelector("span"))).toBe(false);
    expect(isInteractiveTarget(null)).toBe(false);
  });
});
