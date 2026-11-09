/// <reference types="vite/client" />
// 列表渲染的**两面同源**守护（PRD-06-017；纪律见 TDD-15 §3.4a）。
//
// 正文有编辑面（CM6 装饰）与阅读面（React 只读渲染）两套 DOM，它们长得像不像取决于
// 各自 CSS 里写了什么——而"各自写各自的"不会让任何东西变红：真机现场是同一份嵌套列表
// 在阅读面是 `• ○ ▪` 默认色、在编辑面是清一色 accent 圆点，缩进步长一个 20px 一个
// `2ch`（还量在粗体字面上）。本守护钉住"两面消费同一组 Token"这件唯一能机器检查的事。

import { describe, expect, it } from "vitest";

const readingFace = import.meta.glob("../../../../../packages/styles/src/markdown.css", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const editorFace = import.meta.glob("../../../../../packages/styles/src/editor.css", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const tokens = import.meta.glob("../../../../../packages/styles/src/tokens/base.css", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const READING_CSS = Object.values(readingFace)[0];
const EDITOR_CSS = Object.values(editorFace)[0];
const TOKENS_CSS = Object.values(tokens)[0];

/** 两面都必须消费的列表 Token（形状 + 步长 + 字形→正文间隙 + 颜色）。 */
const SHARED_TOKENS = [
  "--editor-list-indent",
  "--editor-list-marker-gap",
  "--editor-list-marker-1",
  "--editor-list-marker-2",
  "--editor-list-marker-3",
  "--editor-list-marker-color",
  "--editor-list-marker-weight",
];

const LIVE_PREVIEW = Object.values(
  import.meta.glob("../../../../../packages/editor-core/src/editor/live-preview.ts", {
    eager: true,
    import: "default",
    query: "?raw",
  }) as Record<string, string>,
)[0];

describe("列表渲染：两面同源", () => {
  it("每个共享 Token 都只在 Token 表里定义一次", () => {
    for (const token of SHARED_TOKENS) {
      const defs = [...TOKENS_CSS.matchAll(new RegExp(`^\\s*${token}\\s*:`, "gm"))];
      expect(defs.length, `${token} 的定义点数量`).toBe(1);
    }
  });

  it("层级步长（缩进 / 圆点占宽）两面共用同一个 Token", () => {
    // 阅读面：列表容器的内缘留白 + `li::before` 悬挂盒定宽；编辑面：圆点 widget 定宽。
    expect(READING_CSS).toContain("padding-inline-start: var(--editor-list-indent)");
    expect(READING_CSS).toContain("inline-size: var(--editor-list-indent)");
    expect(EDITOR_CSS).toContain("inline-size: var(--editor-list-indent)");
  });

  it("阅读面 marker 走 ::before，绝不回到 ::marker", () => {
    // `::marker` 的 `content` 覆盖在 WKWebView 上被静默丢弃：Chromium/无头全绿、
    // 真机退回默认样式——2026-08-23 真机反馈「问题依旧」的成因，禁止复发。
    // 注释里提及不算：只查剥掉注释后的选择器。
    const readingSelectors = READING_CSS.replace(/\/\*[\s\S]*?\*\//gu, "");
    expect(readingSelectors).not.toContain("::marker");
    expect(readingSelectors).toContain("ul > li::before");
  });

  it("字形→正文间隙两面共用同一个 Token（没有它正文会贴着字形）", () => {
    expect(READING_CSS).toContain("padding-inline-end: var(--editor-list-marker-gap)");
    expect(EDITOR_CSS).toContain("padding-inline-end: var(--editor-list-marker-gap)");
  });

  it("marker 字形三档两面都消费", () => {
    for (const token of ["--editor-list-marker-1", "--editor-list-marker-2", "--editor-list-marker-3"]) {
      expect(READING_CSS, `阅读面未消费 ${token}`).toContain(`var(${token})`);
      expect(EDITOR_CSS, `编辑面未消费 ${token}`).toContain(`var(${token})`);
    }
  });

  it("marker 颜色两面共用（编辑面曾单独写 --color-accent）", () => {
    expect(READING_CSS).toContain("var(--editor-list-marker-color)");
    expect(EDITOR_CSS).toContain("var(--editor-list-marker-color)");
  });

  it("marker 字重两面共用（两面曾各写一次字面量 700）", () => {
    expect(READING_CSS).toContain("font-weight: var(--editor-list-marker-weight)");
    expect(EDITOR_CSS).toContain("font-weight: var(--editor-list-marker-weight)");
  });

  /**
   * 有序编号此前**原样保留、毫无着色**：同一篇里"要点"是 accent 加粗圆点、
   * "步骤 1./2./3." 是弱化色语法标记，两种列表的视觉权重不等。
   */
  it("有序编号与无序字形同色同字重（两面）", () => {
    // 阅读面：计数器 marker 走同一组 Token。
    const orderedBlock = /\.luna-markdown ol > li::before\s*\{([^}]*)\}/.exec(READING_CSS);
    expect(orderedBlock, "阅读面的有序编号 marker 规则找不到了").not.toBeNull();
    expect(orderedBlock?.[1]).toContain("var(--editor-list-marker-color)");
    expect(orderedBlock?.[1]).toContain("var(--editor-list-marker-weight)");
    // 编辑面：编号是文档里的真字符，只加一层着色装饰。
    const editorBlock = /\.luna-md-ordered-mark[^{]*\{([^}]*)\}/.exec(EDITOR_CSS);
    expect(editorBlock, "编辑面的有序编号着色规则找不到了").not.toBeNull();
    expect(editorBlock?.[1]).toContain("var(--editor-list-marker-color)");
    expect(editorBlock?.[1]).toContain("var(--editor-list-marker-weight)");
    expect(LIVE_PREVIEW).toContain('Decoration.mark({ class: "luna-md-ordered-mark" })');
  });

  /**
   * 反向条款（有序编号）：阅读面的编号只能由计数器产出。
   * `::marker` 的 `content` 覆盖在 WKWebView 上被静默丢弃（同上一条），而回到原生
   * `list-style: decimal` 就等于放弃着色——两条路都会让这次收敛在真机上无声失效。
   */
  it("阅读面有序编号走 CSS 计数器，不回到原生 marker", () => {
    const readingSelectors = READING_CSS.replace(/\/\*[\s\S]*?\*\//gu, "");
    expect(readingSelectors).toContain("counter-increment: luna-md-ol");
    expect(readingSelectors).toContain("content: counter(luna-md-ol)");
    expect(readingSelectors).toContain("ol > li::before");
    expect(readingSelectors).not.toMatch(/list-style(-type)?:\s*decimal/);
  });

  it("编辑面不把有序编号换成 widget（换掉就不可编辑了）", () => {
    // 无序圆点是 widget（替换 `- `），有序编号只能是 mark——两者的 class 不许混。
    expect(LIVE_PREVIEW).not.toMatch(/new\s+\w*OrderedWidget/);
    expect(LIVE_PREVIEW).toContain("orderedMarks.push");
  });

  /**
   * 反向条款：任一面再写死一个自己的列表尺寸，这条就红。`2ch` 是旧编辑面的圆点定宽，
   * `--space-5` 是旧阅读面的列表内缘留白——它们是这次漂移的两个原始现场。
   */
  it("两面都不再自带一套列表尺寸", () => {
    expect(EDITOR_CSS).not.toMatch(/^\s*inline-size:\s*\d+(\.\d+)?ch;/m);
    const listBlock = /\.luna-markdown ul,\s*\.luna-markdown ol\s*\{([^}]*)\}/.exec(READING_CSS);
    expect(listBlock, "阅读面的列表容器规则找不到了").not.toBeNull();
    expect(listBlock?.[1]).not.toMatch(/padding-inline-start:\s*var\(--space-/);
  });
});
