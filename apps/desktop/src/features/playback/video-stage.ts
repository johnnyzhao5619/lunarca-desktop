/**
 * 视频舞台纯逻辑（PRD-04-058 / PRD-05-023，ADR-003 前端零业务：这里只有呈现规则）。
 *
 * - 布局模式的合法取值与设置字符串的解析（脏值一律回落默认，绝不让 UI 停在未知态）
 * - 字幕叠加内容的选取（跟随回放已有的原文/译文/双语三态，DRY 不另立一套开关）
 * - 悬浮小窗位置的视口夹取（拖动/窗口缩放共用同一函数，绝不出现拖出屏外找不回来）
 *
 * 全部为纯函数，可 headless 测试。
 */

import type { BilingualMode } from "./use-playback";

// ─── 布局模式 ────────────────────────────────────────────────────────────────

/**
 * 视频画面的呈现方式（设置 `playback.video_layout`）：
 * - `split`：与转写列表**并排分屏**（默认——看画面与读/改字幕同时进行，互不遮挡）
 * - `theater`：画面**占满内容区**（转写暂隐，专心看片；回放控制条仍在上方常驻）
 * - `float`：可拖动的**悬浮小窗**（转写占满宽度时仍能瞄一眼画面）
 * - `audio_only`：只听声音（画面隐藏，播放不中断）
 */
export type VideoLayout = "split" | "theater" | "float" | "audio_only";

export const VIDEO_LAYOUTS: readonly VideoLayout[] = [
  "split",
  "theater",
  "float",
  "audio_only",
] as const;

export const DEFAULT_VIDEO_LAYOUT: VideoLayout = "split";

/** 设置值 → 布局模式；未知/脏值回落默认（设置是唯一事实源，但不信任其内容）。 */
export function parseVideoLayout(value: string | null | undefined): VideoLayout {
  return VIDEO_LAYOUTS.includes(value as VideoLayout)
    ? (value as VideoLayout)
    : DEFAULT_VIDEO_LAYOUT;
}

/**
 * 离开同步回放视图后画面的去向（设置 `playback.background_video`，PRD-04-067）：
 * - `audio_only`（默认）：只留声音，「正在播放」条上可随时把画面要回来；
 * - `float`：画面收成可拖动的悬浮小窗跟着走。
 *
 * 它与 `VideoLayout` 是**两件事**：那个说的是"在回放视图里怎么摆"，这个说的是
 * "离开那个视图之后还看不看得见"。合成一个枚举会让"在回放页用剧场、切走后只听声音"
 * 这种再正常不过的组合表达不出来。
 */
export type BackgroundVideo = "audio_only" | "float";

export const BACKGROUND_VIDEOS: readonly BackgroundVideo[] = ["audio_only", "float"] as const;

export const DEFAULT_BACKGROUND_VIDEO: BackgroundVideo = "audio_only";

/** 设置值 → 去向；未知/脏值回落默认。 */
export function parseBackgroundVideo(value: string | null | undefined): BackgroundVideo {
  return BACKGROUND_VIDEOS.includes(value as BackgroundVideo)
    ? (value as BackgroundVideo)
    : DEFAULT_BACKGROUND_VIDEO;
}

/** 字幕字号档（与实时字幕面板同一档位集合，观感一致）。 */
export type SubtitleSize = "small" | "medium" | "large" | "xlarge";

export const SUBTITLE_SIZES: readonly SubtitleSize[] = [
  "small",
  "medium",
  "large",
  "xlarge",
] as const;

export const DEFAULT_SUBTITLE_SIZE: SubtitleSize = "medium";

export function parseSubtitleSize(value: string | null | undefined): SubtitleSize {
  return SUBTITLE_SIZES.includes(value as SubtitleSize)
    ? (value as SubtitleSize)
    : DEFAULT_SUBTITLE_SIZE;
}

// ─── 字幕叠加内容 ────────────────────────────────────────────────────────────

/** 画面上要显示的字幕行（缺失即不渲染该行，绝不留空占位）。 */
export interface SubtitleLines {
  source: string | null;
  translation: string | null;
}

/** 无当前 cue（起播前/间隙）时的空字幕。 */
const EMPTY_SUBTITLE: SubtitleLines = { source: null, translation: null };

/**
 * 由当前 cue + 双语模式选出叠加字幕（与 cue 列表**同一份三态语义**，DRY）：
 * - `source`：只原文
 * - `translation`：只译文；该条**没有译文**时诚实回退原文（否则画面上会凭空丢一句）
 * - `both`：原文 + 译文（有译文才出第二行）
 */
export function subtitleLines(
  cue: { text: string; translation: string | null } | null | undefined,
  mode: BilingualMode,
): SubtitleLines {
  if (!cue) return EMPTY_SUBTITLE;
  const source = cue.text.trim() || null;
  const translation = cue.translation?.trim() || null;
  if (mode === "translation") {
    return translation ? { source: null, translation } : { source, translation: null };
  }
  if (mode === "both") {
    return { source, translation };
  }
  return { source, translation: null };
}

// ─── 悬浮小窗位置 ────────────────────────────────────────────────────────────

export interface FloatPosition {
  x: number;
  y: number;
}

export interface Viewport {
  width: number;
  height: number;
}

export interface FloatSize {
  width: number;
  height: number;
}

/** 悬浮小窗与视口边缘的安全间距（px；与 `--space-4` 同量级，纯逻辑侧不读 CSS）。 */
const FLOAT_MARGIN = 16;

/**
 * 把悬浮小窗位置夹到视口内（拖动中、窗口缩放后共用）。
 *
 * 视口比小窗还小的极端情形下，取 margin 起点而非负值——宁可露出一角，也不能整窗跑到
 * 屏外（那等于用户永远拿不回它，与译境取景框「不可见即视同未放置」同一取舍）。
 */
export function clampFloatPosition(
  position: FloatPosition,
  size: FloatSize,
  viewport: Viewport,
): FloatPosition {
  const maxX = viewport.width - size.width - FLOAT_MARGIN;
  const maxY = viewport.height - size.height - FLOAT_MARGIN;
  return {
    x: Math.round(Math.min(Math.max(position.x, FLOAT_MARGIN), Math.max(FLOAT_MARGIN, maxX))),
    y: Math.round(Math.min(Math.max(position.y, FLOAT_MARGIN), Math.max(FLOAT_MARGIN, maxY))),
  };
}

/** 悬浮小窗的默认停靠点：视口右下角（不挡左侧转写列表与底栏动作）。 */
export function defaultFloatPosition(size: FloatSize, viewport: Viewport): FloatPosition {
  return clampFloatPosition(
    { x: viewport.width - size.width - FLOAT_MARGIN, y: viewport.height - size.height - FLOAT_MARGIN },
    size,
    viewport,
  );
}
