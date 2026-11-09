// 大纲 / 文档目录（纯逻辑，PRD-06-014 P1「大纲 / TOC 导航」）：
// 笔记正文 → 标题层级列表。解析复用编辑器同一 GFM 基线（lezer，代码块内伪标题
// 天然排除）；frontmatter 经阅读模式同源 `splitFrontmatter` 剥离（DRY），
// 行号补偿回原文坐标，直接可用于编辑器跳转。

import { lunaMarkdownParser } from "@lunarca/editor-core/markdown-dialect";
import { syntaxTree } from "@codemirror/language";
import type { EditorState } from "@codemirror/state";
import { splitFrontmatter } from "@lunarca/editor-core/reading-view";

export interface OutlineHeading {
  level: 1 | 2 | 3 | 4 | 5 | 6;
  /** 标题文本（剥离 `#` 标记；行内标记按字面保留）。 */
  text: string;
  /** 原文 1-based 行号（含 frontmatter 偏移，编辑器跳转直接可用）。 */
  line: number;
  /** 正文中第几个标题（0-based；阅读模式按序滚动锚点）。 */
  headingIndex: number;
}

const HEADING_LEVELS: Record<string, 1 | 2 | 3 | 4 | 5 | 6> = {
  ATXHeading1: 1,
  ATXHeading2: 2,
  ATXHeading3: 3,
  ATXHeading4: 4,
  ATXHeading5: 5,
  ATXHeading6: 6,
  SetextHeading1: 1,
  SetextHeading2: 2,
};

/** 解析笔记内容为大纲标题序列。 */
export function parseOutline(content: string): OutlineHeading[] {
  const { body } = splitFrontmatter(content);
  // frontmatter 占用的行数（正文行号 = 正文内行号 + 偏移）。
  const stripped = content.slice(0, content.length - body.length);
  const lineOffset = stripped.length > 0 ? stripped.split("\n").length - 1 : 0;

  const out: OutlineHeading[] = [];
  const tree = lunaMarkdownParser.parse(body);
  tree.iterate({
    enter(node) {
      const level = HEADING_LEVELS[node.name];
      if (!level) {
        return;
      }
      const raw = body.slice(node.from, node.to);
      const firstLine = raw.split("\n", 1)[0];
      const text = firstLine
        .replace(/^#{1,6}(?:[ \t]+|$)/u, "")
        .replace(/[ \t]+#+[ \t]*$/u, "")
        .trim();
      if (text.length === 0) {
        return; // 空标题（裸 `#`）不入大纲。
      }
      const line = body.slice(0, node.from).split("\n").length + lineOffset;
      out.push({ level, text, line, headingIndex: out.length });
    },
  });
  return out;
}

/** 编辑面复用 CM6 增量语法树，避免为每次键入重新解析整篇长文。 */
export function outlineFromState(state: EditorState, frontmatterEnd = 0): OutlineHeading[] {
  return outlineFromTree(state, syntaxTree(state), frontmatterEnd);
}

function outlineFromTree(state: EditorState, tree: ReturnType<typeof syntaxTree>, frontmatterEnd: number): OutlineHeading[] {
  const out: OutlineHeading[] = [];
  tree.iterate({ enter(node) {
    const level = HEADING_LEVELS[node.name];
    if (!level || node.from < frontmatterEnd) return;
    const line = state.doc.lineAt(node.from);
    const text = line.text.replace(/^#{1,6}(?:[ \t]+|$)/u, "").replace(/[ \t]+#+[ \t]*$/u, "").trim();
    if (text) out.push({ level, text, line: line.number, headingIndex: out.length });
  } });
  return out;
}
