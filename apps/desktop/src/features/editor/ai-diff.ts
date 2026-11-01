// 校对差异（PRD-06-021，Apple 写作工具「校对」对标）：原文 vs 改正稿 → 逐条变更。
//
// 为什么在前端算：差异是两段文本之间的确定性计算，不含任何产品判定；CodeMirror 与面板都用
// UTF-16 位置，在这里算就不用跨 IPC 换算偏移。为什么不用结构化输出：2026-09-03 用本地
// qwen3.5:4b 跑 20 段样本，纯文本改正稿 20/20 可用，JSON 结构化 2/20 解析失败且慢 4 倍。
//
// 粒度：拉丁文按词（含前后空白）、CJK 按字、标点单独成 token；LCS 对齐后把相邻的
// 删 / 增合并成一条「把 A 改成 B」。token 太多（超长选区）时退化为整段替换一条。

export interface DiffChange {
  /** 稳定 id（面板里逐条接受 / 拒绝用）。 */
  id: number;
  /** 原文中的 UTF-16 区间 `[from, to)`。 */
  from: number;
  to: number;
  /** 被替换掉的原文片段（空 = 纯插入）。 */
  original: string;
  /** 替换成的文字（空 = 纯删除）。 */
  replacement: string;
}

/** 超过这个 token 数就不逐条算（O(n·m) 的 LCS 会卡住主线程）。 */
export const DIFF_MAX_TOKENS = 1_500;

const TOKEN_PATTERN = /[A-Za-z0-9À-ɏЀ-ӿ']+\s*|\s+|[㐀-鿿豈-﫿぀-ヿ가-힯]|[^\sA-Za-z0-9㐀-鿿]/gu;

function tokenize(text: string): string[] {
  return text.match(TOKEN_PATTERN) ?? [];
}

/** 最长公共子序列的对齐路径：返回每个 token 的操作（equal / delete / insert）。 */
function alignTokens(a: string[], b: string[]): Array<"equal" | "delete" | "insert"> {
  const n = a.length;
  const m = b.length;
  // dp[i][j] = LCS 长度（a[i..], b[j..]），用一维滚动数组按行倒推会丢路径，故保留二维。
  const dp: Uint16Array[] = [];
  for (let i = 0; i <= n; i += 1) {
    dp.push(new Uint16Array(m + 1));
  }
  for (let i = n - 1; i >= 0; i -= 1) {
    for (let j = m - 1; j >= 0; j -= 1) {
      dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }
  const ops: Array<"equal" | "delete" | "insert"> = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      ops.push("equal");
      i += 1;
      j += 1;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      ops.push("delete");
      i += 1;
    } else {
      ops.push("insert");
      j += 1;
    }
  }
  while (i < n) {
    ops.push("delete");
    i += 1;
  }
  while (j < m) {
    ops.push("insert");
    j += 1;
  }
  return ops;
}

/** 原文 → 改正稿的逐条变更；两者相同时为空数组。 */
export function diffChanges(original: string, corrected: string): DiffChange[] {
  if (original === corrected) {
    return [];
  }
  const a = tokenize(original);
  const b = tokenize(corrected);
  if (a.length > DIFF_MAX_TOKENS || b.length > DIFF_MAX_TOKENS) {
    return [{ id: 0, from: 0, to: original.length, original, replacement: corrected }];
  }
  const ops = alignTokens(a, b);
  const changes: DiffChange[] = [];
  let posA = 0;
  let i = 0;
  let j = 0;
  let pending: { from: number; original: string; replacement: string } | null = null;
  const flush = () => {
    if (pending && (pending.original.length > 0 || pending.replacement.length > 0)) {
      changes.push({
        id: changes.length,
        from: pending.from,
        to: pending.from + pending.original.length,
        original: pending.original,
        replacement: pending.replacement,
      });
    }
    pending = null;
  };
  for (const op of ops) {
    if (op === "equal") {
      flush();
      posA += a[i].length;
      i += 1;
      j += 1;
      continue;
    }
    pending ??= { from: posA, original: "", replacement: "" };
    if (op === "delete") {
      pending.original += a[i];
      posA += a[i].length;
      i += 1;
    } else {
      pending.replacement += b[j];
      j += 1;
    }
  }
  flush();
  return changes;
}

/** 只应用被接受的那些变更，得到落地文本。 */
export function applyChanges(
  original: string,
  changes: readonly DiffChange[],
  accepted: ReadonlySet<number>,
): string {
  let out = "";
  let cursor = 0;
  for (const change of changes) {
    out += original.slice(cursor, change.from);
    out += accepted.has(change.id) ? change.replacement : change.original;
    cursor = change.to;
  }
  return out + original.slice(cursor);
}

/** 渲染用的分段：原文顺序展开，变更处带上两侧文本。 */
export type DiffSegment =
  | { kind: "text"; text: string }
  | { kind: "change"; change: DiffChange };

export function diffSegments(original: string, changes: readonly DiffChange[]): DiffSegment[] {
  const segments: DiffSegment[] = [];
  let cursor = 0;
  for (const change of changes) {
    if (change.from > cursor) {
      segments.push({ kind: "text", text: original.slice(cursor, change.from) });
    }
    segments.push({ kind: "change", change });
    cursor = change.to;
  }
  if (cursor < original.length) {
    segments.push({ kind: "text", text: original.slice(cursor) });
  }
  return segments;
}

const SENTENCE_BOUNDARY = /[。！？.!?\n]/u;

/**
 * 含某处变更的那一句（解释一个改动不需要整段出域）：向两侧扩到句末标点或换行为止。
 */
export function sentenceAround(text: string, from: number, to: number): string {
  let start = Math.max(0, Math.min(from, text.length));
  let end = Math.max(start, Math.min(to, text.length));
  while (start > 0 && !SENTENCE_BOUNDARY.test(text[start - 1])) {
    start -= 1;
  }
  while (end < text.length && !SENTENCE_BOUNDARY.test(text[end])) {
    end += 1;
  }
  if (end < text.length) {
    end += 1;
  }
  return text.slice(start, end).trim();
}
