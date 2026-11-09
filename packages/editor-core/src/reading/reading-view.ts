// 阅读模式纯逻辑（PRD-06-017）：frontmatter 属性拆分 + 行内 wiki-link/#tag 切分。
// 与编辑器装饰同构复用同一语法事实源（`WIKI_LINK_RE` / `scanLineForTags`），
// 保证三种视图模式对同一份 md 文本的语义一致。

import { WIKI_LINK_RE } from "./wiki-link-syntax";
import { scanLineForTags } from "./inline-tags";

/**
 * 阅读面里的正文标题，按文档顺序——大纲与 wiki 片段给的 `headingIndex` 就是这里的下标。
 * 只数 Markdown 正文里的标题：frontmatter「属性」块自带一个小标题，算进去会让有属性的
 * 笔记每次都跳到上一个标题。
 */
export function readingHeadings(container: ParentNode): HTMLElement[] {
  const selector = [1, 2, 3, 4, 5, 6].map((level) => `.luna-markdown h${level}`).join(", ");
  return Array.from(container.querySelectorAll<HTMLElement>(selector));
}

// ─── Frontmatter 属性（Obsidian Reading view「Properties」对标） ─────────────

export interface FrontmatterSplit {
  /** 原始 YAML 文本（不含 `---` 围栏）；无 frontmatter 时为 null。 */
  frontmatter: string | null;
  /** 去除 frontmatter 后的正文。 */
  body: string;
}

/** 拆出文档头部 YAML frontmatter（仅识别首行起始的 `---` 围栏，与 CM 折叠扩展同界定）。 */
export function splitFrontmatter(content: string): FrontmatterSplit {
  if (!content.startsWith("---\n") && content !== "---") {
    return { frontmatter: null, body: content };
  }
  const close = content.indexOf("\n---", 3);
  if (close === -1) {
    return { frontmatter: null, body: content };
  }
  const after = close + "\n---".length;
  // 围栏闭合行之后必须是行尾/文件尾，否则不是合法 frontmatter。
  if (after < content.length && content[after] !== "\n") {
    return { frontmatter: null, body: content };
  }
  return {
    frontmatter: content.slice("---\n".length, close),
    body: content.slice(Math.min(after + 1, content.length)),
  };
}

export interface FrontmatterEntry {
  key: string;
  value: string;
}

/**
 * 「属性」块不呈现的顶层键：`luna` 是 Lunarca 自己维护的机器元数据命名空间
 * （schema / type / stage / topic / synth_body / ai_tags …）——它的每一项在界面上
 * 都已有专属呈现面（页头徽标、待整理小标、标签树的 ✨AI 分组），在属性块里以
 * 浅解析原样漏出来只会变成一行读不懂的键值汤（真机第八轮：产物页顶着一行
 * 「luna: <一串标签>」）。用户自己的 frontmatter 键（`tags:` / `title:` …）照常展示。
 */
const HIDDEN_FRONTMATTER_KEYS = new Set(["luna"]);

/**
 * 展示级 YAML 顶层键值提取（只读呈现，不做完整 YAML 解析、绝不改写事实源）：
 * `key: value` 行成对；紧随其后的 `- item` 列表行聚合为逗号分隔值。
 * 被隐藏键（上表）连同其缩进子行整段跳过——子行的列表项不得串进上一个可见键。
 */
export function frontmatterEntries(frontmatter: string): FrontmatterEntry[] {
  const out: FrontmatterEntry[] = [];
  let hiddenTail = false;
  for (const line of frontmatter.split("\n")) {
    const listItem = /^\s+-\s+(.*)$/u.exec(line);
    if (listItem) {
      if (hiddenTail || out.length === 0) {
        continue;
      }
      const last = out[out.length - 1];
      last.value = last.value ? `${last.value}, ${listItem[1].trim()}` : listItem[1].trim();
      continue;
    }
    const pair = /^([A-Za-z0-9_][\w./-]*)\s*:\s*(.*)$/u.exec(line);
    if (pair) {
      hiddenTail = HIDDEN_FRONTMATTER_KEYS.has(pair[1]);
      if (!hiddenTail) {
        out.push({ key: pair[1], value: pair[2].trim() });
      }
      continue;
    }
    // 缩进的嵌套 `key: value` 行（顶层正则不吃）：归属最近的顶层键——隐藏态原样延续。
  }
  return out;
}

// ─── 行内 wiki-link / #tag 切分 ──────────────────────────────────────────────

export type ReadingInlinePart =
  | { kind: "text"; text: string }
  | { kind: "wiki"; target: string; label: string }
  | { kind: "tag"; tag: string };

/** 单段文本（已在代码区之外）切分为 纯文本 / wiki-link / 标签 三类部件。 */
export function splitReadingInline(text: string): ReadingInlinePart[] {
  const out: ReadingInlinePart[] = [];
  WIKI_LINK_RE.lastIndex = 0;
  let cursor = 0;
  let match: RegExpExecArray | null;
  while ((match = WIKI_LINK_RE.exec(text)) !== null) {
    if (match.index > cursor) {
      pushTextWithTags(out, text.slice(cursor, match.index));
    }
    const target = match[1].trim();
    if (target) {
      out.push({ kind: "wiki", target, label: (match[2] ?? match[1]).trim() });
    } else {
      pushTextWithTags(out, match[0]);
    }
    cursor = match.index + match[0].length;
  }
  if (cursor < text.length) {
    pushTextWithTags(out, text.slice(cursor));
  }
  return out;
}

function pushTextWithTags(out: ReadingInlinePart[], text: string) {
  // 逐行跑与编辑器装饰同一套 #tag 扫描（行首/空白前缀规则依赖行上下文）。
  let offset = 0;
  for (const line of text.split("\n")) {
    const lineStart = offset;
    let cursor = 0;
    for (const match of scanLineForTags(line)) {
      if (match.from > cursor) {
        pushText(out, text.slice(lineStart + cursor, lineStart + match.from));
      }
      out.push({ kind: "tag", tag: match.tag });
      cursor = match.to;
    }
    if (cursor < line.length) {
      pushText(out, line.slice(cursor));
    }
    offset += line.length + 1;
    if (offset <= text.length) {
      pushText(out, "\n");
    }
  }
  // 末尾多推的换行回收（split 产物比换行数多一段）。
  const last = out[out.length - 1];
  if (last?.kind === "text" && last.text === "\n" && !text.endsWith("\n")) {
    out.pop();
  }
}

function pushText(out: ReadingInlinePart[], text: string) {
  if (!text) {
    return;
  }
  const last = out[out.length - 1];
  if (last?.kind === "text") {
    last.text += text;
    return;
  }
  out.push({ kind: "text", text });
}
