/**
 * 宿主能力端口（TDD-17 §2）：渲染器只说「要做什么」，怎么做归宿主——桌面经 IPC，网页经 Worker
 * 与浏览器。端口经 `MarkdownContent` 的 `host` 属性显式传入（必填：漏传是类型错误，不是运行时
 * 才发现的空操作）。
 *
 * 与具体一篇笔记、一个库相关的能力（wiki 链接解析与打开、库内图片地址、库内相对链接、嵌入）
 * 不在这里，而是 `MarkdownContent` 的 `renderText` / `renderImage` / `openVaultLink` /
 * `renderEmbed` 这几个按次传入的渲染钩子——没有库的场合（对话、AI 结果）本来就不给它们。
 */

import type { ComponentType } from "react";

export interface MarkdownHost {
  /**
   * 这条链接能不能做成可点击的外链；不放行的照纯文本显示（防 scheme 注入）。
   * 桌面放行 http / https（`isSafeExternalUrl`）；网页只放行 https 与 mailto（TDD-17 §6.2）。
   */
  isExternalUrlAllowed(href: string): boolean;
  /** 打开一条已经放行的外链（桌面交系统浏览器；网页开新标签）。 */
  openExternalUrl(href: string): void;
  /**
   * HTML / SVG 字符串进 DOM 的**唯一出口**（KaTeX 与 Mermaid 的输出）：返回值原样交给
   * `dangerouslySetInnerHTML`。桌面原样返回；网页在 CSP `require-trusted-types-for 'script'`
   * 下包一层 Trusted Types policy（TDD-17 §6.1）。包内只有 `HostHtml` 调用它。
   */
  toTrustedHtml(html: string): string | TrustedHTML;
  /**
   * 代码块头条的「复制」按钮。复制走哪条剪贴板通道、按钮用哪套 UI 原语都是宿主的事；
   * 桌面传它与编辑面代码块共用的同一个组件（TDD-15 §3.4a）。
   */
  CodeCopyButton: ComponentType<{ code: string }>;
  /**
   * 公式的输出形态：`html` 是 KaTeX 的 HTML 排版（带行内样式，桌面）；`mathml` 只出浏览器原生排版的
   * MathML——网页的 CSP `style-src 'self'` 不允许行内样式（TDD-17 §6.1），HTML 排版在那里会缺样式、
   * 并逐个记成违规。
   */
  mathMarkup: "html" | "mathml";
  /**
   * Mermaid 图表怎么画（PRD-21-012：网页与桌面看到的是同一张图）：
   * - `inline`：包内 Mermaid 画成内联 SVG（桌面）；
   * - `image`：交宿主画成图片——网页在 Trusted Types 与 `style-src 'self'` 下内联不了 Mermaid（它内部写
   *   innerHTML、SVG 里自带 `<style>`），改由不透明来源的沙箱页画、主页面以 `<img>` 显示（TDD-17 §6.1）。
   */
  diagrams: { kind: "inline" } | ImageDiagrams;
}

/** 图表的明暗：与 Mermaid 的主题名一致。 */
export type DiagramTheme = "default" | "dark";

/**
 * 画成图片的结果。`image` 的 `url` 归调用方：不用了（换笔记、卸载、换主题）必须调一次 `release`。
 * `error` 的 `message` 是给人看的原因（Mermaid 的语法说明，或宿主的说明）。
 */
export type DiagramImage = { kind: "image"; url: string; release(): void } | { kind: "error"; message: string };

export interface ImageDiagrams {
  kind: "image";
  /** 画一段图表源码；只 resolve、不 reject——失败一律给 `error`。 */
  render(source: string, theme: DiagramTheme): Promise<DiagramImage>;
}
