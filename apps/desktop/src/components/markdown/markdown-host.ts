/**
 * 桌面对共享渲染包（`@lunarca/editor-core`）的宿主端口实现（TDD-17 §2）。渲染面每一处
 * `MarkdownContent` 都传这一份，行为与渲染器搬进包之前一致：
 * - 外链只放行 http(s)，经 Rust `open_external_url` 交系统浏览器（协议白名单在 Rust，这里是前置校验）；
 * - 桌面 webview 不开 Trusted Types，HTML 原样交给 React；
 * - 代码块复制按钮与编辑面代码块是同一个组件（TDD-15 §3.4a）。
 */

import type { MarkdownHost } from "@lunarca/editor-core/host";
import { isSafeExternalUrl } from "@lunarca/editor-core/markdown";
import { commands } from "../../ipc/bindings";
import { CodeCopyButton } from "./CodeCopyButton";

export const desktopMarkdownHost: MarkdownHost = {
  isExternalUrlAllowed: isSafeExternalUrl,
  openExternalUrl: (href) => {
    void commands.openExternalUrl(href);
  },
  toTrustedHtml: (html) => html,
  CodeCopyButton,
  mathMarkup: "html",
  diagrams: { kind: "inline" },
};
