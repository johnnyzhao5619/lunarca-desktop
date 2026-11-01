/**
 * 实时字幕视图模型（录音 HUD 与实时字幕面板共用，DRY）。
 *
 * 把「已定稿转录段 + 逐句译文单元 + 暂定尾部」编织为**双语字幕 cue**（原文一行、译文一行，
 * 影片字幕/歌词式）。译文按其覆盖区间 `[startMs, endMs]` **重组源句**（而非逐段精确配对），
 * 从根上避免"译文所属段滚出可视区就再不显示"，也天然把跨段的一句合并为一条 cue。
 */

import type { HypothesisLine, TranscriptLine, TranslationEntry } from "./use-recording";

/** 一条双语字幕 cue（原文 + 译文；译文未就绪或未开转译时为 null）。 */
export interface SubtitleCue {
  /** cue 锚点（句首 ms），作 React key 与排序。 */
  startMs: number;
  /** 源句（区间内 committed 转录段重组）。 */
  source: string;
  /** 译文（null = 译文待生成 / 未开启转译）。 */
  translation: string | null;
  /** 轨级说话人标签（"self"/"remote"；null=无标签）。双语 cue 取区间内首行的轨。 */
  track?: string | null;
}

/** 字幕视图：最近若干 cue + 各轨暂定尾部（灰显、可被改写）。 */
export interface SubtitleView {
  cues: SubtitleCue[];
  hypotheses: HypothesisLine[];
}

/**
 * 应用一条轨级 hypothesis 全量快照。空文本只清该轨；非空更新移到数组尾部，数组顺序因此
 * 同时表达“最近更新”的沉浸显示优先级。该投影是 Both 两路互不覆盖的前端不变量。
 */
export function updateTrackHypothesis(
  current: HypothesisLine[],
  next: HypothesisLine,
): HypothesisLine[] {
  const track = next.track ?? null;
  const otherTracks = current.filter((entry) => (entry.track ?? null) !== track);
  return next.text ? [...otherTracks, { text: next.text, track }] : otherTracks;
}

/**
 * 电影式沉浸字幕只呈现当前语义单元：有正在变化的 hypothesis 时显示最近更新的轨，待其
 * 清空后回落到最近一条 committed cue。框体模式仍可同时呈现两轨当前暂定尾，且二者绝不
 * 互相覆盖。
 * 历史仍由框体模式与完整会话状态持有，沉浸层不把多句堆成转录面板。
 */
export function focusSubtitleView(view: SubtitleView): SubtitleView {
  const latestHypothesis = view.hypotheses[view.hypotheses.length - 1];
  if (latestHypothesis) return { cues: [], hypotheses: [latestHypothesis] };
  const latest = view.cues[view.cues.length - 1];
  return latest ? { cues: [latest], hypotheses: [] } : view;
}

/**
 * 只显示译文的那扇窗（译文独立窗）的沉浸焦点：**最新一条已经有译文的 cue**。
 *
 * 不能复用 `focusSubtitleView`：说话时它只留下正在听写的暂定句，而暂定句还没有译文——
 * 一扇只显示译文的窗于是整窗空白，直到停嘴（真机 2026-10-01 设计复核坐实）。
 */
export function focusTranslatedView(view: SubtitleView): SubtitleView {
  for (let index = view.cues.length - 1; index >= 0; index -= 1) {
    const cue = view.cues[index];
    if (cue.translation !== null && cue.translation.trim() !== "") {
      return { cues: [cue], hypotheses: [] };
    }
  }
  return { cues: [], hypotheses: [] };
}

function sameTrack(left: string | null | undefined, right: string | null | undefined): boolean {
  return (left ?? null) === (right ?? null);
}

/** 取同一音轨覆盖区间 `[startMs, endMs]` 内的转录段并重组为源句。 */
function joinRange(
  transcript: TranscriptLine[],
  startMs: number,
  endMs: number,
  track: string | null | undefined,
): string {
  return transcript
    .filter(
      (line) =>
        sameTrack(line.track, track) && line.startMs >= startMs && line.startMs <= endMs,
    )
    .map((line) => line.text)
    .join(" ")
    .trim();
}

/**
 * 构造字幕视图（纯函数，headless 可测）。
 *
 * - 无译文（未开转译 / 尚未产出）：取最近 `maxCues` 条转录段作单语源 cue。
 * - 有译文：每条译文单元 → 一条双语 cue（源句按**同轨**区间重组）；没有被同轨译文覆盖
 *   的 committed 段保留为单语 cue。两类按时间轴合并，仅保留最近 `maxCues` 条。
 */
export function buildSubtitleView(
  transcript: TranscriptLine[],
  hypotheses: HypothesisLine[],
  translations: TranslationEntry[],
  maxCues: number,
): SubtitleView {
  const limit = Math.max(1, maxCues);

  if (translations.length === 0) {
    const start = Math.max(0, transcript.length - limit);
    const cues: SubtitleCue[] = transcript.slice(start).map((line) => ({
      startMs: line.startMs,
      source: line.text,
      translation: null,
      track: line.track ?? null,
    }));
    return { cues, hypotheses };
  }

  const translatedCues: SubtitleCue[] = translations.map((entry) => ({
    startMs: entry.startMs,
    source: joinRange(transcript, entry.startMs, entry.endMs, entry.track),
    translation: entry.text,
    track: entry.track ?? null,
  }));

  const sourceOnlyCues: SubtitleCue[] = transcript
    .filter(
      (line) =>
        !translations.some(
          (entry) =>
            sameTrack(line.track, entry.track) &&
            line.startMs >= entry.startMs &&
            line.startMs <= entry.endMs,
        ),
    )
    .map((line) => ({
      startMs: line.startMs,
      source: line.text,
      translation: null,
      track: line.track ?? null,
    }));

  const cues = [...translatedCues, ...sourceOnlyCues].sort((a, b) => a.startMs - b.startMs);
  const start = Math.max(0, cues.length - limit);
  return { cues: cues.slice(start), hypotheses };
}
