/**
 * 对话消息 Markdown 渲染：委托共享 `MarkdownContent`（单一解析/渲染基线，DRY）。
 * 对话场景不做行内定制：图片按外链呈现（默认离线纪律，不远程加载）。
 *
 * 唯一的定制点是**引用角标**（PRD-16-005）：把回答里的 `[n]` 变成可点按钮。它走
 * `MarkdownContent` 既有的 `renderText` 扩展点（阅读模式用同一个点做 wiki-link 与
 * `#tag`），不为引用另起一套渲染管线。
 */

import type { ReactNode } from "react";
import { MarkdownContent } from "../../components/markdown/MarkdownContent";

export interface ChatMarkdownProps {
  content: string;
  /** 流式进行中：末尾追加闪烁打字光标（PRD-03 验收）。 */
  streaming?: boolean;
  /** 行内纯文本定制渲染（引用角标）；返回 null 即走默认原样文本。 */
  renderText?: (text: string, key: number) => ReactNode | null;
}

export function ChatMarkdown({ content, streaming = false, renderText }: ChatMarkdownProps) {
  return <MarkdownContent content={content} streaming={streaming} renderText={renderText} />;
}
