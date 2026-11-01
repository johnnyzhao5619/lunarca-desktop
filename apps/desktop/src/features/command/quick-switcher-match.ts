/**
 * 快速切换器的匹配与排序（PRD-07-040）。纯函数，可无头测试。
 *
 * 四档，档间不交叉：
 * 1. **空查询**列最近打开（新在前）；还没有最近打开时列全部（首次使用不给一块空面板）；
 * 2. 标题 / 别名 / 路径的模糊匹配；
 * 3. 拼音命中（`hy` → 会议）：前缀命中优于包含，整档排在模糊匹配之后——用户打的若正是
 *    某个英文标题，它不该被一串拼音碰巧对上的中文标题挤下去；
 * 4. 正文命中（后端全文搜索的相关度次序）。
 *
 * 资料区（ADR-024）在各档内降权、不排除。拼音键由 Rust 生成（`luna_vault::pinyin_keys`），
 * 这里只比对字符串。
 */

import { fuzzyMatch, type FuzzyMatch } from "./fuzzy";

/** 切换器的一条（`SwitcherEntryDto` 加上前端拼好的检索串与正文片段）。 */
export interface QuickSwitcherItem {
  path: string;
  title: string;
  /** 标题 + 别名 + 路径：模糊匹配的对象。 */
  searchText: string;
  pinyin: readonly string[];
  isSourceMaterial: boolean;
  /** 全文搜索命中的上下文片段（仅内容命中项有值）。 */
  snippet?: string;
}

export interface SwitcherRanks {
  /** 路径 → 最近打开的名次（0 = 最近）。 */
  recent: ReadonlyMap<string, number>;
  /** 路径 → 正文命中的名次。 */
  content: ReadonlyMap<string, number>;
}

/** 拼音档的基准分：低于任何模糊匹配（每个字符至少 10 分），高于正文档。 */
const PINYIN_BASE_SCORE = 0;

/** 内容命中排在所有标题命中之后的基准分。 */
const CONTENT_MATCH_BASE_SCORE = -1000;

/**
 * 资料区的降权幅度（ADR-024）：小于 `|CONTENT_MATCH_BASE_SCORE|`，降权只在同一档内让位，
 * 不会把一条标题命中压到正文命中那一档之下。
 */
const SOURCE_MATERIAL_PENALTY = 500;

/** 拼音键里找查询：前缀 2 分、包含 1 分；查询不是纯字母数字（打的是汉字）就不比。 */
function pinyinScore(query: string, keys: readonly string[]): number | null {
  const needle = query.toLowerCase().replace(/\s+/g, "");
  if (!/^[a-z0-9]+$/.test(needle)) {
    return null;
  }
  let best: number | null = null;
  for (const key of keys) {
    const score = key.startsWith(needle) ? 2 : key.includes(needle) ? 1 : null;
    if (score !== null && (best === null || score > best)) {
      best = score;
    }
  }
  return best;
}

export function matchSwitcherItem(
  item: QuickSwitcherItem,
  query: string,
  ranks: SwitcherRanks,
): FuzzyMatch | null {
  const penalty = item.isSourceMaterial ? SOURCE_MATERIAL_PENALTY : 0;
  if (!query.trim()) {
    if (ranks.recent.size === 0) {
      return { score: -penalty, ranges: [] };
    }
    const rank = ranks.recent.get(item.path);
    return rank === undefined ? null : { score: -rank, ranges: [] };
  }
  const textMatch = fuzzyMatch(query, item.searchText);
  if (textMatch) {
    return { ...textMatch, score: textMatch.score - penalty };
  }
  const pinyin = pinyinScore(query, item.pinyin);
  if (pinyin !== null) {
    return { score: PINYIN_BASE_SCORE + pinyin - penalty, ranges: [] };
  }
  const rank = ranks.content.get(item.path);
  if (rank !== undefined) {
    return { score: CONTENT_MATCH_BASE_SCORE - rank - penalty, ranges: [] };
  }
  return null;
}
