/**
 * 系统通道的降级投影（PRD-15-001 / PRD-15-012）：发不出去的事件必须落回 toast，
 * 且不再带 `system`；没有 `system` 的事件原样不动。
 */

import { describe, expect, it } from "vitest";
import type { NotificationEventDto } from "../../ipc/bindings";
import { degradeToToast, routedToSystem } from "./notification-system";
import { visibleNotificationChannel } from "./notification-routing";

function event(channels: string[]): NotificationEventDto {
  return {
    id: "n1",
    class: "completion",
    severity: "info",
    channels,
    target: null,
    i18n_key: "notifications:transcribe_fetched",
    params: {},
    diagnostic_i18n_key: null,
  };
}

describe("系统通知通道", () => {
  it("不在场的完成类带 system，在场的不带", () => {
    expect(routedToSystem(["system", "status"])).toBe(true);
    expect(routedToSystem(["toast", "status"])).toBe(false);
  });

  it("发不出去 → 去掉 system、补上 toast，宿主按 toast 呈现", () => {
    const degraded = degradeToToast(event(["system", "status"]));
    expect(degraded.channels).not.toContain("system");
    expect(visibleNotificationChannel(degraded.channels)).toBe("toast");
  });

  it("已经带 toast 的不会重复补", () => {
    const degraded = degradeToToast(event(["system", "toast"]));
    expect(degraded.channels.filter((c) => c === "toast")).toHaveLength(1);
  });
});
