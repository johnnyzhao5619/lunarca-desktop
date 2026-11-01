export type MatchRange = readonly [start: number, end: number];

export interface FuzzyMatch {
  score: number;
  ranges: MatchRange[];
}

function isBoundary(candidate: string, index: number): boolean {
  if (index === 0) {
    return true;
  }
  const previous = candidate[index - 1];
  return previous === "/" || previous === "\\" || previous === "-" || previous === "_" || previous === " ";
}

function compactRanges(indices: number[]): MatchRange[] {
  if (indices.length === 0) {
    return [];
  }

  const ranges: MatchRange[] = [];
  let start = indices[0];
  let previous = indices[0];

  for (const index of indices.slice(1)) {
    if (index === previous + 1) {
      previous = index;
      continue;
    }
    ranges.push([start, previous + 1]);
    start = index;
    previous = index;
  }

  ranges.push([start, previous + 1]);
  return ranges;
}

export function fuzzyMatch(query: string, candidate: string): FuzzyMatch | null {
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const normalizedCandidate = candidate.toLocaleLowerCase();

  if (normalizedQuery.length === 0) {
    return { score: 0, ranges: [] };
  }

  const indices: number[] = [];
  let searchFrom = 0;

  for (const char of normalizedQuery) {
    const index = normalizedCandidate.indexOf(char, searchFrom);
    if (index === -1) {
      return null;
    }
    indices.push(index);
    searchFrom = index + 1;
  }

  let score = 0;
  for (let i = 0; i < indices.length; i += 1) {
    const index = indices[i];
    score += 10;

    if (isBoundary(candidate, index)) {
      score += 8;
    }

    if (i > 0) {
      const gap = index - indices[i - 1];
      if (gap === 1) {
        score += 12;
      } else {
        score -= Math.min(gap - 1, 12);
      }
    }
  }

  score -= Math.min(candidate.length - normalizedQuery.length, 40) * 0.1;

  return {
    score,
    ranges: compactRanges(indices),
  };
}

export function highlightParts(value: string, ranges: readonly MatchRange[]) {
  const parts: Array<{ text: string; highlighted: boolean }> = [];
  let cursor = 0;

  for (const [start, end] of ranges) {
    if (start > cursor) {
      parts.push({ text: value.slice(cursor, start), highlighted: false });
    }
    parts.push({ text: value.slice(start, end), highlighted: true });
    cursor = end;
  }

  if (cursor < value.length) {
    parts.push({ text: value.slice(cursor), highlighted: false });
  }

  return parts;
}
