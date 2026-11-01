import { describe, expect, it } from "vitest";
import type { NotificationEventDto } from "../../ipc/bindings";
import {
  autoDismissMs,
  collapseNotificationStack,
  stackNotificationIds,
  TOAST_DISMISS_MS,
  TOAST_FAILURE_DISMISS_MS,
  visibleNotificationChannel,
} from "./notification-routing";

function event(
  overrides: Partial<NotificationEventDto> & Pick<NotificationEventDto, "id">,
): NotificationEventDto {
  return {
    class: "completion",
    severity: "info",
    channels: ["toast", "status"],
    target: null,
    i18n_key: "notifications:done",
    params: {},
    diagnostic_i18n_key: null,
    ...overrides,
  };
}

describe("notification routing", () => {
  // 「已加入队列 N 件」回执（PRD-06-051 ⑩ 修订）是信息类 toast：走完成/信息档时长，
  // 不是失败档，更不是持久 banner——此前它是一条只能手动关的底部常驻条。
  it("info-class toast auto-dismisses on the completion/info tier", () => {
    expect(
      autoDismissMs(event({ id: "queued", class: "info", channels: ["toast", "status"] })),
    ).toBe(TOAST_DISMISS_MS);
  });

  it("renders a multi-channel event in one visible lane only", () => {
    expect(visibleNotificationChannel(["inline", "toast"])).toBe("toast");
  });

  it("keeps banner as the highest priority visible lane", () => {
    expect(visibleNotificationChannel(["inline", "toast", "banner"])).toBe("banner");
  });

  // inline 恒不入宿主：带 inline 的两类路由同时带 toast/banner，就地提示由各功能自渲染。
  // system/status 亦然：前者是 OS 横幅通道，后者是角标，都不由宿主呈现。
  it("never surfaces inline / system / status lanes in the notification host", () => {
    expect(visibleNotificationChannel(["inline"])).toBeNull();
    expect(visibleNotificationChannel(["system", "status"])).toBeNull();
  });

  // PRD-15-011：toast 瞬时非阻断，失败类给更长阅读时间；PRD-15-020：banner 持久不消散。
  it("scales toast TTL by class and keeps banners persistent", () => {
    expect(autoDismissMs(event({ id: "1" }))).toBe(TOAST_DISMISS_MS);
    expect(autoDismissMs(event({ id: "2", class: "failure" }))).toBe(TOAST_FAILURE_DISMISS_MS);
    expect(autoDismissMs(event({ id: "3", channels: ["banner", "status"] }))).toBeNull();
    expect(autoDismissMs(event({ id: "4", channels: ["banner", "toast"] }))).toBeNull();
    expect(autoDismissMs(event({ id: "5", channels: ["system", "status"] }))).toBeNull();
  });
});

describe("同类折叠与数量封顶（PRD-15-031）", () => {
  const health = (id: string, key = "notifications:mic_silent") =>
    event({ id, class: "system_health", severity: "critical", channels: ["banner", "status"], i18n_key: key });

  it("同一件事重复上报折叠成一行并计数", () => {
    const stack = collapseNotificationStack([health("3"), health("2"), health("1")], "banner");
    expect(stack.visible).toHaveLength(1);
    expect(stack.visible[0].count).toBe(3);
    expect(stack.visible[0].item.id).toBe("3"); // 代表条取最新
    expect(stack.overflow).toBe(0);
  });

  it("深链不同视为不同事项，不误合并", () => {
    const a = event({ id: "a", channels: ["banner"], target: "settings://permissions" });
    const b = event({ id: "b", channels: ["banner"], target: "settings://notes" });
    expect(collapseNotificationStack([a, b], "banner").visible).toHaveLength(2);
  });

  it("超出上限的折叠为「还有 N 条」，计的是条数不是组数", () => {
    const items = [
      health("6", "k6"),
      health("5", "k5"),
      health("4", "k4"),
      health("3", "k3"),
      health("2", "k3"),
    ];
    const stack = collapseNotificationStack(items, "banner", 3);
    expect(stack.visible.map((v) => v.item.id)).toEqual(["6", "5", "4"]);
    expect(stack.overflow).toBe(2); // 第 4 组折叠了 2 条
  });

  it("只收本通道的条目", () => {
    const toast = event({ id: "t" });
    const banner = health("b");
    expect(collapseNotificationStack([toast, banner], "toast").visible).toHaveLength(1);
    expect(stackNotificationIds([toast, banner], "toast")).toEqual(["t"]);
    expect(stackNotificationIds([toast, banner], "banner")).toEqual(["b"]);
  });
});
