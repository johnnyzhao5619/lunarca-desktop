/**
 * 脚本工具栏「AI 改稿」没有选区时改哪一段（ADR-165 ⑦）：光标所在的那一段——从本段标题行到下一段
 * 标题行之前，段尾的空白不算（替换回来时不吞掉段与段之间的空行）。
 *
 * 段界只认 Rust 分段给出的标题行（`ScriptOutlineDto.sections[].line`，1 起、含 frontmatter，与
 * 编辑器行号同一口径）；这里只是把「第几行」换算成文档里的字符偏移，不另判什么算一段。
 */

export interface TextRange {
  from: number;
  to: number;
}

/** `starts`：各段起始行（含开场那一段的行）；光标在第一段之前或文档为空时为 `null`。 */
export function scriptSectionRangeAt(text: string, starts: readonly number[], cursor: number): TextRange | null {
  const lineStarts = [0];
  for (let index = 0; index < text.length; index += 1) {
    if (text[index] === "\n") lineStarts.push(index + 1);
  }
  const lineCount = lineStarts.length;
  const valid = [...new Set(starts)].filter((line) => line >= 1 && line <= lineCount).sort((a, b) => a - b);
  const clamped = Math.max(0, Math.min(cursor, text.length));
  let cursorLine = 1;
  while (cursorLine < lineCount && lineStarts[cursorLine] <= clamped) cursorLine += 1;
  const start = [...valid].reverse().find((line) => line <= cursorLine);
  if (start === undefined) return null;
  const next = valid.find((line) => line > start);
  const from = lineStarts[start - 1];
  let to = next === undefined ? text.length : lineStarts[next - 1];
  while (to > from && /\s/.test(text[to - 1])) to -= 1;
  return to > from ? { from, to } : null;
}
