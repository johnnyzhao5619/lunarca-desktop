/**
 * 回放视图内查找（PRD-04-028 补记）：在**已加载的整篇 cue** 里按子串定位——这是"找回那句话"，
 * 不是全库搜索；命中不隐藏别的行（转写的上下文就是价值），只高亮 + 在命中之间跳。
 *
 * 纯函数、零 IPC：cue 已在内存，这里只是呈现层的定位，不算业务（ADR-003）。
 */

import type { BilingualMode } from "./use-playback";

export interface FindableCue {
  text: string;
  translation: string | null;
}

function normalize(value: string): string {
  return value.toLocaleLowerCase();
}

/** 命中的 cue 下标（升序）。空 / 全空白查询恒为空数组。 */
export function findCueMatches(
  cues: readonly FindableCue[],
  query: string,
  mode: BilingualMode,
): number[] {
  const needle = normalize(query.trim());
  if (!needle) return [];
  const out: number[] = [];
  cues.forEach((cue, index) => {
    // 只在**看得见的那一层**里找：仅译文模式下原文没显示，命中它会跳到一行没高亮的字。
    const inSource = mode !== "translation" || !cue.translation;
    const inTranslation = mode !== "source" && Boolean(cue.translation);
    if (
      (inSource && normalize(cue.text).includes(needle)) ||
      (inTranslation && normalize(cue.translation ?? "").includes(needle))
    ) {
      out.push(index);
    }
  });
  return out;
}

/** 把一段文字按命中切成片段（`hit` = 该片段是命中）；无查询时整段一片。 */
export function splitByMatch(text: string, query: string): Array<{ text: string; hit: boolean }> {
  const needle = normalize(query.trim());
  if (!needle) return [{ text, hit: false }];
  const lower = normalize(text);
  // 归一化后长度可能与原文不一致（个别字符大小写映射会变长）；那种情况下退回不高亮，
  // 宁可少一处高亮也不能切错字。
  if (lower.length !== text.length) return [{ text, hit: false }];
  const parts: Array<{ text: string; hit: boolean }> = [];
  let cursor = 0;
  for (;;) {
    const at = lower.indexOf(needle, cursor);
    if (at < 0) break;
    if (at > cursor) parts.push({ text: text.slice(cursor, at), hit: false });
    parts.push({ text: text.slice(at, at + needle.length), hit: true });
    cursor = at + needle.length;
  }
  if (cursor < text.length) parts.push({ text: text.slice(cursor), hit: false });
  return parts.length > 0 ? parts : [{ text, hit: false }];
}

/** 在 `total` 个命中里环绕步进；没有命中恒为 -1。 */
export function stepMatch(current: number, total: number, delta: 1 | -1): number {
  if (total <= 0) return -1;
  if (current < 0) return delta === 1 ? 0 : total - 1;
  return (current + delta + total) % total;
}
