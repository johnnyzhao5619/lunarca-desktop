/**
 * 视频舞台纯逻辑测试（PRD-04-058）：布局解析 / 字幕三态 / 悬浮小窗夹取。
 */

import { describe, expect, it } from "vitest";
import {
  clampFloatPosition,
  defaultFloatPosition,
  parseBackgroundVideo,
  parseSubtitleSize,
  parseVideoLayout,
  subtitleLines,
} from "./video-stage";

describe("布局与字号设置解析", () => {
  it("合法值原样通过，脏值/缺省一律回落默认", () => {
    expect(parseVideoLayout("split")).toBe("split");
    expect(parseVideoLayout("float")).toBe("float");
    expect(parseVideoLayout("audio_only")).toBe("audio_only");
    expect(parseVideoLayout("pip")).toBe("split");
    expect(parseVideoLayout(null)).toBe("split");
    expect(parseVideoLayout(undefined)).toBe("split");

    expect(parseSubtitleSize("xlarge")).toBe("xlarge");
    expect(parseSubtitleSize("")).toBe("medium");
  });

  it("切走后画面去向：默认只留声音（PRD-04-067）", () => {
    expect(parseBackgroundVideo("float")).toBe("float");
    expect(parseBackgroundVideo("audio_only")).toBe("audio_only");
    // 脏值回落到"只留声音"而不是"悬浮小窗"：默认必须落在**不打扰**的那一侧，
    // 一块自动弹出来的画面会盖住用户刚要写的地方。
    expect(parseBackgroundVideo("theater")).toBe("audio_only");
    expect(parseBackgroundVideo(null)).toBe("audio_only");
  });
});

describe("字幕叠加内容（与 cue 列表同一份三态语义）", () => {
  const bilingual = { text: "Hello world", translation: "你好，世界" };
  const sourceOnly = { text: "Hello world", translation: null };

  it("原文态只出原文", () => {
    expect(subtitleLines(bilingual, "source")).toEqual({
      source: "Hello world",
      translation: null,
    });
  });

  it("译文态只出译文；该条无译文时诚实回退原文", () => {
    expect(subtitleLines(bilingual, "translation")).toEqual({
      source: null,
      translation: "你好，世界",
    });
    expect(subtitleLines(sourceOnly, "translation")).toEqual({
      source: "Hello world",
      translation: null,
    });
  });

  it("双语态两行并出；无译文只剩原文", () => {
    expect(subtitleLines(bilingual, "both")).toEqual({
      source: "Hello world",
      translation: "你好，世界",
    });
    expect(subtitleLines(sourceOnly, "both")).toEqual({
      source: "Hello world",
      translation: null,
    });
  });

  it("无当前 cue / 空白文本 → 不渲染任何行", () => {
    expect(subtitleLines(null, "both")).toEqual({ source: null, translation: null });
    expect(subtitleLines({ text: "   ", translation: "  " }, "both")).toEqual({
      source: null,
      translation: null,
    });
  });
});

describe("悬浮小窗位置", () => {
  const size = { width: 320, height: 200 };
  const viewport = { width: 1280, height: 800 };

  it("默认停靠右下角且留出安全间距", () => {
    expect(defaultFloatPosition(size, viewport)).toEqual({ x: 944, y: 584 });
  });

  it("拖出视口即被夹回可见范围", () => {
    expect(clampFloatPosition({ x: -500, y: -500 }, size, viewport)).toEqual({ x: 16, y: 16 });
    expect(clampFloatPosition({ x: 9999, y: 9999 }, size, viewport)).toEqual({ x: 944, y: 584 });
  });

  it("视口比小窗还小时仍留在左上可见角，绝不给负坐标", () => {
    expect(clampFloatPosition({ x: 100, y: 100 }, size, { width: 200, height: 120 })).toEqual({
      x: 16,
      y: 16,
    });
  });
});
