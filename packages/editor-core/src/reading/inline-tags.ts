// 正文内联 `#tag` 的语法（PRD-06-033）：编辑面装饰、阅读面切分、网页端标签扫描共用这一份
// （TDD-17 §5.6）。规则与 Rust `extract_inline_tags` 同构。

export interface InlineTagMatch {
  /** 相对行首偏移（含 `#`）。 */
  from: number;
  to: number;
  tag: string;
}

function isTagChar(ch: string): boolean {
  return /[\p{L}\p{N}_/-]/u.test(ch);
}

/**
 * 扫描单行内联标签。规则与 Rust `extract_inline_tags` 同构（单一语义两端实现，
 * 索引在 Rust、装饰在前端）：`#` 前须行首/空白；至少一个非数字字符；跳过行内 code span。
 */
export function scanLineForTags(lineText: string): InlineTagMatch[] {
  const out: InlineTagMatch[] = [];
  let inCodeSpan = false;
  let prev: string | null = null;
  let i = 0;
  while (i < lineText.length) {
    const ch = lineText[i];
    if (ch === "`") {
      inCodeSpan = !inCodeSpan;
      prev = ch;
      i += 1;
      continue;
    }
    if (!inCodeSpan && ch === "#" && (prev === null || /\s/u.test(prev))) {
      let end = i + 1;
      while (end < lineText.length && isTagChar(lineText[end])) {
        end += 1;
      }
      // 尾部 `/` 不计入标签体；首字符为 `/` 视为非标签（与 Rust extract_inline_tags 同构）。
      let bodyEnd = end;
      while (bodyEnd > i + 1 && lineText[bodyEnd - 1] === "/") {
        bodyEnd -= 1;
      }
      const raw = lineText.slice(i + 1, bodyEnd);
      if (raw && !raw.startsWith("/") && /\D/u.test(raw)) {
        out.push({ from: i, to: bodyEnd, tag: raw });
        prev = lineText[end - 1] ?? ch;
        i = end;
        continue;
      }
    }
    prev = ch;
    i += 1;
  }
  return out;
}
