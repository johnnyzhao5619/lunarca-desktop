/**
 * 包内 HTML 字符串落 DOM 的唯一出口：内容一律先经宿主的 `toTrustedHtml`（TDD-17 §6.1）。
 * `html-sink-guard.test.ts` 钉住包里别处不许再出现 `innerHTML` 一类写法。
 *
 * 走 `dangerouslySetInnerHTML` 而不是挂载后命令式赋值：静态导出与打印用
 * `renderToStaticMarkup`，那里不跑副作用，命令式赋值的结果进不了导出物。
 */

import type { MarkdownHost } from "../host";

export interface HostHtmlProps {
  host: MarkdownHost;
  /** 已由可信生成器产出的 HTML / SVG（KaTeX `trust: false`、Mermaid `securityLevel: "strict"`）。 */
  html: string;
  as: "div" | "span";
  className: string;
}

export function HostHtml({ host, html, as: Tag, className }: HostHtmlProps) {
  return <Tag className={className} dangerouslySetInnerHTML={{ __html: host.toTrustedHtml(html) }} />;
}
