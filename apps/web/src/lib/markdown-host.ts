/**
 * 网页对共享渲染包的宿主端口（TDD-17 §2 / §6.2）：
 * - 外链只放行 `https:` 与 `mailto:`，新标签打开、不带 opener 与 referrer；其余（含 `http:`、
 *   `javascript:`、`data:`）由渲染器按纯文本显示；
 * - HTML（KaTeX 的输出）经名为 `lunarca` 的 Trusted Types policy 进 DOM；
 * - 公式只出 MathML：CSP `style-src 'self'` 不许行内样式（TDD-17 §6.1）；
 * - Mermaid 交沙箱页画成图片（`diagram-sandbox.ts`，PRD-21-012）。
 */

import type { MarkdownHost } from "@lunarca/editor-core/host";
import { CodeCopyButton } from "../features/reader/CodeCopyButton";
import i18n from "../i18n/i18n";
import { createDiagramSandbox, createSandboxFrame } from "./diagram-sandbox";
import { trustedHtml } from "./trusted-types";

const ALLOWED_EXTERNAL = /^(?:https:\/\/|mailto:)/i;

export function isWebExternalUrlAllowed(href: string): boolean {
  return ALLOWED_EXTERNAL.test(href.trim());
}

const webDiagrams = createDiagramSandbox({
  createFrame: createSandboxFrame,
  host: window,
  createObjectURL: (blob) => URL.createObjectURL(blob),
  revokeObjectURL: (url) => URL.revokeObjectURL(url),
  font: () => getComputedStyle(document.body).fontFamily,
  timeoutMessage: () => i18n.t("reader.diagramTimeout"),
});

export const webMarkdownHost: MarkdownHost = {
  isExternalUrlAllowed: isWebExternalUrlAllowed,
  openExternalUrl: (href) => {
    if (isWebExternalUrlAllowed(href)) window.open(href.trim(), "_blank", "noopener,noreferrer");
  },
  toTrustedHtml: trustedHtml,
  CodeCopyButton,
  mathMarkup: "mathml",
  diagrams: webDiagrams,
};
