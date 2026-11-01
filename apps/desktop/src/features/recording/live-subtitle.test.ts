import { describe, expect, it } from "vitest";
import { buildSubtitleView, focusSubtitleView, focusTranslatedView, updateTrackHypothesis, type SubtitleView } from "./live-subtitle";

describe("updateTrackHypothesis", () => {
  it("clearing the muted microphone lane preserves the live system lane", () => {
    const both = [
      { text: "麦克风暂定", track: "self" },
      { text: "系统声音暂定", track: "remote" },
    ];
    expect(updateTrackHypothesis(both, { text: "", track: "self" })).toEqual([
      { text: "系统声音暂定", track: "remote" },
    ]);
  });

  it("updates one lane without overwriting the other and moves it to the newest position", () => {
    const current = [
      { text: "系统旧文本", track: "remote" },
      { text: "麦克风文本", track: "self" },
    ];
    expect(updateTrackHypothesis(current, { text: "系统新文本", track: "remote" })).toEqual([
      { text: "麦克风文本", track: "self" },
      { text: "系统新文本", track: "remote" },
    ]);
  });
});

describe("buildSubtitleView", () => {
  it("shows monolingual source cues when there are no translations", () => {
    const view = buildSubtitleView(
      [
        { startMs: 0, text: "你好世界" },
        { startMs: 1000, text: "今天天气不错" },
      ],
      [],
      [],
      4,
    );
    expect(view.cues).toEqual([
      { startMs: 0, source: "你好世界", translation: null, track: null },
      { startMs: 1000, source: "今天天气不错", translation: null, track: null },
    ]);
  });

  it("reassembles a source sentence from its coverage range and pairs the translation", () => {
    const view = buildSubtitleView(
      [
        { startMs: 0, text: "今天我们" },
        { startMs: 500, text: "讨论三个议题" },
      ],
      [],
      [{ startMs: 0, endMs: 500, text: "Today we discuss three topics", track: null }],
      4,
    );
    expect(view.cues).toEqual([
      { startMs: 0, source: "今天我们 讨论三个议题", translation: "Today we discuss three topics", track: null },
    ]);
  });

  it("shows committed segments after the last translation as an in-progress (untranslated) cue", () => {
    const view = buildSubtitleView(
      [
        { startMs: 0, text: "第一句。" },
        { startMs: 1000, text: "正在说的第二句" },
      ],
      [],
      [{ startMs: 0, endMs: 0, text: "First sentence." }],
      4,
    );
    expect(view.cues).toEqual([
      { startMs: 0, source: "第一句。", translation: "First sentence.", track: null },
      { startMs: 1000, source: "正在说的第二句", translation: null, track: null },
    ]);
  });

  it("keeps only the most recent maxCues cues (translation stays visible)", () => {
    const transcript = Array.from({ length: 10 }, (_, i) => ({ startMs: i, text: `L${i}` }));
    const translations = Array.from({ length: 10 }, (_, i) => ({
      startMs: i,
      endMs: i,
      text: `T${i}`,
    }));
    const view = buildSubtitleView(transcript, [], translations, 3);
    expect(view.cues.map((c) => c.translation)).toEqual(["T7", "T8", "T9"]);
  });

  it("carries track-scoped hypothesis tails through unchanged", () => {
    const hypotheses = [
      { text: "我正在说……", track: "self" },
      { text: "对方正在说……", track: "remote" },
    ];
    const view = buildSubtitleView([], hypotheses, [], 4);
    expect(view.hypotheses).toEqual(hypotheses);
    expect(view.cues).toHaveLength(0);
  });
});

describe("focusSubtitleView", () => {
  it("有新暂定尾时优先显示最近更新的轨，不让旧定稿遮住低延迟字幕", () => {
    const view = focusSubtitleView({
      cues: [
        { startMs: 0, source: "上一句", translation: null },
        { startMs: 1, source: "当前句", translation: "Current sentence" },
      ],
      hypotheses: [{ text: "下一句暂定", track: "remote" }],
    });
    expect(view).toEqual({
      cues: [],
      hypotheses: [{ text: "下一句暂定", track: "remote" }],
    });
  });

  it("暂定尾清空后只保留最新定稿 cue", () => {
    expect(focusSubtitleView({
      cues: [
        { startMs: 0, source: "上一句", translation: null },
        { startMs: 1, source: "当前句", translation: "Current sentence" },
      ],
      hypotheses: [],
    })).toEqual({
      cues: [{ startMs: 1, source: "当前句", translation: "Current sentence" }],
      hypotheses: [],
    });
  });

  it("尚无定稿时只保留最近更新的轨级 hypothesis", () => {
    expect(focusSubtitleView({
      cues: [],
      hypotheses: [
        { text: "我正在说", track: "self" },
        { text: "系统正在说", track: "remote" },
      ],
    })).toEqual({
      cues: [],
      hypotheses: [{ text: "系统正在说", track: "remote" }],
    });
  });
});

// ── 轨级说话人标签透传（PRD-04-053 L0，Both 双轨）─────────────────────────────

describe("track passthrough", () => {
  it("单语 cue 透传行级 track", () => {
    const view = buildSubtitleView(
      [
        { startMs: 0, text: "hello", track: "self" },
        { startMs: 1000, text: "world", track: "remote" },
      ],
      [],
      [],
      10,
    );
    expect(view.cues.map((c) => c.track)).toEqual(["self", "remote"]);
  });

  it("双语 cue 取区间内首行 track；无标签为 null", () => {
    const view = buildSubtitleView(
      [
        { startMs: 0, text: "你好", track: "remote" },
        { startMs: 3000, text: "后续", track: null },
      ],
      [],
      [{ startMs: 0, endMs: 2000, text: "hello", track: "remote" }],
      10,
    );
    expect(view.cues[0].track).toBe("remote");
    expect(view.cues[1].track).toBeNull();
  });

  it("双源时间区间重叠时译文只与同轨原文配对", () => {
    const view = buildSubtitleView(
      [
        { startMs: 0, text: "我方原文", track: "self" },
        { startMs: 0, text: "对方原文", track: "remote" },
      ],
      [],
      [{ startMs: 0, endMs: 1000, text: "Remote translation", track: "remote" }],
      10,
    );
    expect(view.cues).toEqual([
      {
        startMs: 0,
        source: "对方原文",
        translation: "Remote translation",
        track: "remote",
      },
      { startMs: 0, source: "我方原文", translation: null, track: "self" },
    ]);
  });
});

describe("focusTranslatedView（译文独立窗的沉浸焦点）", () => {
  it("说话中（有暂定句）仍给最新一条有译文的 cue，而不是空白", () => {
    const view = {
      cues: [
        { startMs: 0, source: "早", translation: "Morning" },
        { startMs: 1000, source: "今天", translation: null },
      ],
      hypotheses: [{ text: "我们开始", track: null }],
    } as unknown as SubtitleView;
    expect(focusTranslatedView(view)).toEqual({
      cues: [{ startMs: 0, source: "早", translation: "Morning" }],
      hypotheses: [],
    });
  });

  it("一条译文都还没有时如实给空（由空态文案说「等待译文」）", () => {
    const view = { cues: [{ startMs: 0, source: "早", translation: null }], hypotheses: [] } as unknown as SubtitleView;
    expect(focusTranslatedView(view)).toEqual({ cues: [], hypotheses: [] });
  });
});
