// Markdown 语法高亮（PRD-06-011 就地渲染基线 + PRD-06-016 GFM 方言）。
// 一律输出**语义 class**，颜色/字号在共享样式包里用设计 Token 定义（TDD-15 禁硬编码），
// 替换 CM6 defaultHighlightStyle（其内置色不随 Lunarca 主题走）。
//
// 住在 `highlight/` 而不是 `editor/`，因为**两个渲染面共用同一份 tag→class 映射**
// （TDD-15 §3.4a）：编辑面经 `syntaxHighlighting()` 由 CM6 消费，阅读面（`render/
// MarkdownContent`）经 `code-highlight.ts` 的 `highlightCodeSpans()` 直接跑 `highlightTree`。
// 此前只有编辑面着色，同一段代码编辑态彩色、阅读态是一整片 `--code-text` 单色。

import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags } from "@lezer/highlight";
import type { Extension } from "@codemirror/state";

/** 导出供端到端高亮回归测试（highlightTree × 本样式断言 luna-tok-* 产出）。 */
export const lunaHighlightStyle = HighlightStyle.define([
  { tag: tags.heading1, class: "luna-md-h1" },
  { tag: tags.heading2, class: "luna-md-h2" },
  { tag: tags.heading3, class: "luna-md-h3" },
  { tag: tags.heading4, class: "luna-md-h4" },
  { tag: tags.heading5, class: "luna-md-h5" },
  { tag: tags.heading6, class: "luna-md-h6" },
  // 通用 heading（HeaderMark `#`、GFM 表格 TableHeader 单元格）。
  { tag: tags.heading, class: "luna-md-heading" },
  { tag: tags.strong, class: "luna-md-strong" },
  { tag: tags.emphasis, class: "luna-md-em" },
  { tag: tags.strikethrough, class: "luna-md-del" },
  { tag: tags.monospace, class: "luna-md-code" },
  { tag: tags.quote, class: "luna-md-quote" },
  { tag: tags.link, class: "luna-md-link" },
  { tag: tags.url, class: "luna-md-url" },
  // **`tags.list` 刻意不映射任何样式**：lezer-markdown 把它挂在**整个 ListItem** 上
  // （"这段文字在列表里"），不是挂在项目符号上。曾把它映射成 `.luna-md-list-mark`
  // （accent 色 + 600 字重），于是**每篇以要点为主的笔记，正文的每一个字都被按项目符号
  // 上色加粗**——AI 蒸馏产物几乎全是要点列表，等于产品的核心产出面整片染色。
  // 项目符号自己不需要它：Live Preview 下 `*` 被换成 `BulletWidget`（`.luna-md-bullet`
  // 自带 accent + 700），源码模式下它随 `tags.processingInstruction` 与 `**`、`#`、`|`
  // 一样走 `.luna-md-mark`。`tags.list` 无法区分"符号"与"项内正文"（两者都带它），
  // 故给它任何视觉都必然波及正文——这条不是调参数，是它压根不该有样式。
  { tag: tags.atom, class: "luna-md-atom" },
  { tag: tags.labelName, class: "luna-md-label" },
  { tag: tags.contentSeparator, class: "luna-md-hr" },
  { tag: tags.escape, class: "luna-md-escape" },
  { tag: tags.comment, class: "luna-md-comment" },
  { tag: tags.meta, class: "luna-md-meta" },
  // 语法标记字符（`**`、`` ` ``、表格分隔 `|---|` 等）。
  { tag: tags.processingInstruction, class: "luna-md-mark" },
  // ── 围栏代码块内嵌语言 token（PRD-06-011/016，Typora 对标；语言解析由
  // @codemirror/language-data 按 CodeInfo 惰性加载）。颜色 = styles/index.css --syntax-*。
  { tag: tags.keyword, class: "luna-tok-keyword" },
  { tag: [tags.string, tags.special(tags.string), tags.regexp], class: "luna-tok-string" },
  { tag: [tags.number, tags.bool, tags.null], class: "luna-tok-number" },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName)], class: "luna-tok-func" },
  { tag: [tags.typeName, tags.className, tags.namespace], class: "luna-tok-type" },
  { tag: tags.propertyName, class: "luna-tok-prop" },
  { tag: [tags.operator, tags.punctuation], class: "luna-tok-op" },
  { tag: [tags.lineComment, tags.blockComment, tags.docComment], class: "luna-md-comment" },
]);

/** Lunarca 主题化 Markdown 高亮扩展（含表格表头/删除线/任务等 GFM 节点）。 */
export function lunaSyntaxHighlighting(): Extension {
  return syntaxHighlighting(lunaHighlightStyle, { fallback: true });
}
