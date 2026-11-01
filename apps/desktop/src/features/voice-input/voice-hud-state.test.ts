import { describe, expect, it } from "vitest";
import type { VoiceInputState } from "../../ipc/bindings";
import {
  applyVoiceModePatch,
  buildVoicePrimaryAction,
  nextEditedVoiceOutput,
  nextLivePartial,
  nextTranscriptFeedback,
  estimateTextRows,
  shouldResetVoiceLevels,
  voiceLandingActions,
  voiceFeedbackLabelKey,
  voiceStatusTone,
} from "./voice-hud-state";

function state(status: VoiceInputState["status"]): VoiceInputState {
  return {
    status,
    landing_mode: "inject",
    input_mode: "hold",
    auto_refine: true,
    translate_enabled: false,
    translate_target_lang: "en",
    message_key: null,
    message_detail: null,
    result_pending: false,
  };
}

describe("voice HUD state projection", () => {
  it("starts from idle instead of rendering an inert finish button", () => {
    expect(buildVoicePrimaryAction(state("idle"))).toEqual({
      kind: "start",
      labelKey: "action.start",
      disabled: false,
    });
  });

  it("finishes only while capturing; processing renders a disabled processing action", () => {
    expect(buildVoicePrimaryAction(state("recording"))).toEqual({
      kind: "stop",
      labelKey: "action.stop",
      disabled: false,
    });
    expect(buildVoicePrimaryAction(state("recognizing"))).toEqual({
      kind: "processing",
      labelKey: "action.processing",
      disabled: true,
    });
    expect(buildVoicePrimaryAction(state("landing"))).toEqual({
      kind: "processing",
      labelKey: "action.processing",
      disabled: true,
    });
  });

  it("clears previous transcript feedback on cancel, but not on a mid-session Recording refresh", () => {
    const previous = nextTranscriptFeedback(null, {
      kind: "transcript",
      data: { raw: "hello", final_text: "Hello.", refined: true, output_kind: "refined" },
    });

    expect(previous?.finalText).toBe("Hello.");
    // Rust 在会话进行中反复重发 Recording 态（电平诊断 / 等额度）；那不是新会话。
    expect(nextTranscriptFeedback(previous, { kind: "state", data: state("recording") })).toBe(
      previous,
    );
    expect(nextTranscriptFeedback(previous, { kind: "state", data: state("cancelled") })).toBeNull();
  });

  it("resets voice levels on cancel / error only — a Recording refresh must not blink the meter", () => {
    expect(shouldResetVoiceLevels({ kind: "state", data: state("recording") })).toBe(false);
    expect(shouldResetVoiceLevels({ kind: "state", data: state("cancelled") })).toBe(true);
    expect(shouldResetVoiceLevels({ kind: "state", data: state("error") })).toBe(true);
    expect(shouldResetVoiceLevels({ kind: "state", data: state("idle") })).toBe(false);
    // 最终结果到达即归零：结果态不再冻结最后几帧（看起来像还在录）。
    expect(
      shouldResetVoiceLevels({
        kind: "transcript",
        data: { raw: "a", final_text: "a", refined: false, output_kind: "raw" },
      }),
    ).toBe(true);
  });

  it("keeps output destination as default while exposing explicit landing actions", () => {
    expect(voiceLandingActions("quick_note")).toEqual([
      { mode: "inject", labelKey: "landing.inject", isDefault: false },
      { mode: "clipboard", labelKey: "landing.clipboard", isDefault: false },
      { mode: "quick_note", labelKey: "landing.quick_note", isDefault: true },
    ]);
  });

  it("maps statuses to stable presentation tones", () => {
    expect(voiceStatusTone("idle")).toBe("ready");
    expect(voiceStatusTone("recording")).toBe("live");
    expect(voiceStatusTone("recognizing")).toBe("processing");
    expect(voiceStatusTone("error")).toBe("danger");
  });


  it("labels translated feedback separately from polish", () => {
    const feedback = nextTranscriptFeedback(null, {
      kind: "transcript",
      data: {
        raw: "我做得不是很好。我制定了一个计划，想问一些问题。",
        final_text: "I didn't do it very well. I made a plan and wanted to ask questions.",
        refined: true,
        output_kind: "translated",
      },
    });

    expect(feedback?.outputKind).toBe("translated");
    expect(voiceFeedbackLabelKey(feedback)).toBe("preview.translated");
  });

  it("keeps edited output after landing fails", () => {
    const feedback = nextTranscriptFeedback(null, {
      kind: "transcript",
      data: { raw: "draft", final_text: "Polished draft", refined: true, output_kind: "refined" },
    });

    expect(nextEditedVoiceOutput("edited result", feedback, feedback)).toBe("edited result");
  });

  it("updates one voice option without resetting other choices", () => {
    const current: VoiceInputState = {
      ...state("idle"),
      landing_mode: "clipboard",
      input_mode: "toggle",
      auto_refine: false,
      translate_enabled: true,
      translate_target_lang: "ja",
    };

    expect(applyVoiceModePatch(current, { inputMode: "hold" })).toMatchObject({
      landingMode: "clipboard",
      inputMode: "hold",
      autoRefine: false,
      translateEnabled: true,
      translateTargetLang: "ja",
    });
    expect(applyVoiceModePatch(current, { landingMode: "quick_note" })).toMatchObject({
      landingMode: "quick_note",
      inputMode: "toggle",
    });
  });

  // PRD-02-004：按住说话期间实时识别反馈的状态机。
  it("replaces live partial on each partial event", () => {
    const first = nextLivePartial(null, {
      kind: "partial",
      data: { text: "你好", hypothesis: "世" },
    });
    expect(first).toEqual({ text: "你好", hypothesis: "世" });

    const second = nextLivePartial(first, {
      kind: "partial",
      data: { text: "你好 世界", hypothesis: "" },
    });
    expect(second).toEqual({ text: "你好 世界", hypothesis: "" });
  });

  it("keeps live partial across mid-session Recording refreshes; clears on cancel or final transcript", () => {
    // 真机症状：几个字刚出现就消失、再出现——每条电平诊断都让 Rust 重发一次 Recording 态，
    // 而这里曾把每一次都当成"新会话开始"清空。新会话的清空归 `startVoice`。
    const live = { text: "hello", hypothesis: "wor" };
    expect(nextLivePartial(live, { kind: "state", data: state("recording") })).toBe(live);
    expect(nextLivePartial(live, { kind: "state", data: state("cancelled") })).toBeNull();
    expect(
      nextLivePartial(live, {
        kind: "transcript",
        data: { raw: "hello world", final_text: "Hello world.", refined: true, output_kind: "refined" },
      }),
    ).toBeNull();
    // 识别/落地等中间态保持实时文字（松开后继续滚动到 Finished）。
    expect(nextLivePartial(live, { kind: "state", data: state("recognizing") })).toBe(live);
  });
});

describe("estimateTextRows：按宽度单位折行（R-23）", () => {
  const opts = { unitsPerLine: 56, min: 3, max: 8 };
  it("空文本与短句取下限", () => {
    expect(estimateTextRows("", opts)).toBe(3);
    expect(estimateTextRows("hello", opts)).toBe(3);
  });
  it("中文一个字占两个单位：28 个汉字正好一行，56 个汉字两行——此前按 42 字符估会给出两倍", () => {
    expect(estimateTextRows("一".repeat(28), { unitsPerLine: 56, min: 1, max: 8 })).toBe(1);
    expect(estimateTextRows("一".repeat(56), { unitsPerLine: 56, min: 1, max: 8 })).toBe(2);
    expect(estimateTextRows("a".repeat(56), { unitsPerLine: 56, min: 1, max: 8 })).toBe(1);
  });
  it("换行各自折行，并封顶", () => {
    expect(estimateTextRows("a\nb\nc\nd", { unitsPerLine: 56, min: 1, max: 8 })).toBe(4);
    expect(estimateTextRows("一".repeat(1000), opts)).toBe(8);
  });
});
