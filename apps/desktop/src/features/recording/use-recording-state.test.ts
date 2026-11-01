import { describe, expect, it } from "vitest";
import { isRecordingActive, isRecordingSessionOpen } from "./use-recording";

describe("recording session visibility", () => {
  it("keeps stopping inside the open session shell", () => {
    expect(isRecordingActive("stopping")).toBe(false);
    expect(isRecordingSessionOpen("stopping")).toBe(true);
  });

  it("only treats recording, paused and stopping as an open session", () => {
    expect(
      ["idle", "recording", "paused", "stopping", "mic_busy", "error"].filter((status) =>
        isRecordingSessionOpen(status as Parameters<typeof isRecordingSessionOpen>[0]),
      ),
    ).toEqual(["recording", "paused", "stopping"]);
  });
});
