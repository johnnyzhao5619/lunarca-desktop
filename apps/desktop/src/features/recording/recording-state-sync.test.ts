import { describe, expect, it } from "vitest";
import type { RecordingState } from "../../ipc/bindings";
import { acceptsBroadcastState, crossesSessionBoundary } from "./recording-state-sync";

function snapshot(partial: Partial<RecordingState>): RecordingState {
  return {
    status: "recording",
    purpose: "record",
    session_id: "s1",
    note_rel: null,
    elapsed_ms: 0,
    message_key: null,
    message_detail: null,
    ...partial,
  };
}

describe("录音表面以控制器广播为准（H2）", () => {
  it("同形态的会话态采纳，异形态的不采纳——字幕在跑时录音 HUD 不能画成录音中", () => {
    expect(acceptsBroadcastState("record", snapshot({ purpose: "record" }))).toBe(true);
    expect(acceptsBroadcastState("record", snapshot({ purpose: "caption" }))).toBe(false);
    expect(acceptsBroadcastState("caption", snapshot({ purpose: "caption" }))).toBe(true);
    expect(acceptsBroadcastState("caption", snapshot({ purpose: "record" }))).toBe(false);
  });

  it("idle 恒采纳：它是「异常结束 → 复位」要送达每一扇窗的那一条", () => {
    // `RecordingState::idle()` 的 purpose 恒为 record——字幕面板也必须收下它。
    const idle = snapshot({ status: "idle", purpose: "record", session_id: null });
    expect(acceptsBroadcastState("record", idle)).toBe(true);
    expect(acceptsBroadcastState("caption", idle)).toBe(true);
  });
});

describe("横幅生命周期绑定会话实例（M2）", () => {
  const live = snapshot({ status: "recording", session_id: "s1" });

  it("会话结束（idle / error）即跨界", () => {
    expect(crossesSessionBoundary(live, snapshot({ status: "idle", session_id: null }))).toBe(true);
    expect(crossesSessionBoundary(live, snapshot({ status: "error", session_id: "s1" }))).toBe(true);
    expect(
      crossesSessionBoundary(
        snapshot({ status: "stopping", session_id: "s1" }),
        snapshot({ status: "idle", session_id: null }),
      ),
    ).toBe(true);
  });

  it("换了一场会话即跨界；同一场内的暂停/停止中不算", () => {
    expect(crossesSessionBoundary(live, snapshot({ status: "recording", session_id: "s2" }))).toBe(
      true,
    );
    expect(crossesSessionBoundary(live, snapshot({ status: "paused", session_id: "s1" }))).toBe(
      false,
    );
    expect(crossesSessionBoundary(live, snapshot({ status: "stopping", session_id: "s1" }))).toBe(
      false,
    );
  });

  it("重复收到同一份终态不算再跨一次（广播与 Channel 两条路会各送一遍）", () => {
    const idle = snapshot({ status: "idle", session_id: null });
    expect(crossesSessionBoundary(idle, idle)).toBe(false);
  });
});
