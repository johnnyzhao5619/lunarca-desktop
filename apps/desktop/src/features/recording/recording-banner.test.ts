import { describe, expect, it } from "vitest";
import { pickRecordingBanner, type RecordingBannerInput } from "./recording-banner";

function input(overrides: Partial<RecordingBannerInput> = {}): RecordingBannerInput {
  return {
    transcribeStopped: null,
    transcribeReconnecting: false,
    translateStopped: null,
    translateEnabled: true,
    degradedTier: null,
    degradedReason: null,
    ...overrides,
  };
}

describe("录音 HUD 横幅只挂一条（PRD-04-003 修订）", () => {
  it("什么都没发生时没有横幅", () => {
    expect(pickRecordingBanner(input())).toEqual({ shown: null, folded: [] });
  });

  it("三条同时发生时只上屏最高优先级的一条，其余折叠", () => {
    const { shown, folded } = pickRecordingBanner(
      input({
        transcribeStopped: "QUOTA_EXHAUSTED",
        translateStopped: "MODEL_MISSING",
        degradedTier: "red",
        degradedReason: "window_drop",
      }),
    );
    expect(shown?.kind).toBe("transcribe_stopped");
    expect(shown?.key).toBe("transcribe_stopped.quota_exhausted");
    expect(folded.map((b) => b.kind)).toEqual(["translate_stopped", "degraded"]);
  });

  it("转写掉线压过重连中：两条同时挂着会互相矛盾", () => {
    const { shown, folded } = pickRecordingBanner(
      input({ transcribeStopped: "TIMEOUT", transcribeReconnecting: true }),
    );
    expect(shown?.kind).toBe("transcribe_stopped");
    expect(folded).toEqual([]);
  });

  it("转译关着时「转译停了」不算一条", () => {
    const { shown } = pickRecordingBanner(
      input({ translateStopped: "MODEL_MISSING", translateEnabled: false }),
    );
    expect(shown).toBeNull();
  });

  it("负载降级：绿档不算；有原因码取原因文案，兜底回档位文案", () => {
    expect(pickRecordingBanner(input({ degradedTier: "green" })).shown).toBeNull();
    const withReason = pickRecordingBanner(
      input({ degradedTier: "yellow", degradedReason: "admission_wait" }),
    ).shown;
    expect(withReason).toMatchObject({
      kind: "degraded",
      tier: "yellow",
      key: "degraded.admission_wait",
      fallbackKey: "degraded.yellow",
    });
    expect(pickRecordingBanner(input({ degradedTier: "red" })).shown?.key).toBe("degraded.red");
  });
});
