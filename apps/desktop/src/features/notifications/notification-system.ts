/**
 * 系统通知通道（PRD-15-012）。
 *
 * Rust 按在场态决定一条事件带不带 `system` 通道；这里负责**真的发出去**——文案用与应用内
 * 提示同一份 `notifications.json`（不在 Rust 侧再存一份），经 `tauri-plugin-notification`
 * 交给 macOS 通知中心。发不出去（用户在系统设置里关了 / 平台不支持 / 插件报错）就把这条
 * 事件**降级为应用内 toast**（PRD-15-001：拒绝通知权限不致功能不可用，绝不静默丢失）。
 *
 * 只发「类型 + 计数 + 状态」级别的文案：事件参数由后端脱敏后下发，正文类内容从不进 DTO
 * （PRD-15-050），系统横幅的泄露面因此不高于应用内提示。
 */

import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import type { NotificationEventDto } from "../../ipc/bindings";
import { i18n } from "../../i18n/i18n";
import { resolveNotificationBaseKey } from "./notification-i18n";

/** 一条事件是否被 Rust 路由到了系统通道。 */
export function routedToSystem(channels: readonly string[]): boolean {
  return channels.includes("system");
}

/** 降级：去掉 `system`，补上 `toast`（幂等），让宿主按应用内规约呈现。 */
export function degradeToToast(item: NotificationEventDto): NotificationEventDto {
  const channels = item.channels.filter((channel) => channel !== "system");
  if (!channels.includes("toast")) {
    channels.push("toast");
  }
  return { ...item, channels };
}

/**
 * 发出系统通知；返回**呈现给宿主的事件**：发成功 = 原样（宿主不再画 toast，历史照记）；
 * 发不出去 = 降级后的事件。没有 `system` 通道的事件原样返回，零副作用。
 */
export async function deliverSystemNotification(
  item: NotificationEventDto,
): Promise<NotificationEventDto> {
  if (!routedToSystem(item.channels)) {
    return item;
  }
  try {
    let granted = await isPermissionGranted();
    if (!granted) {
      // 首次触发即授权（PRD-15-001）：能弹系统授权框的平台在此弹；macOS 桌面无此框，
      // 插件如实回 granted，真正的开关在「系统设置 › 通知」。
      granted = (await requestPermission()) === "granted";
    }
    if (!granted) {
      return degradeToToast(item);
    }
    const baseKey = resolveNotificationBaseKey(item.i18n_key, i18n);
    sendNotification({
      title: i18n.t(`${baseKey}.title`, { ns: "notifications", ...item.params }),
      body: i18n.t(`${baseKey}.body`, { ns: "notifications", ...item.params }),
    });
    return item;
  } catch {
    return degradeToToast(item);
  }
}
