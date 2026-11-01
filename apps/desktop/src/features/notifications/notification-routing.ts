/**
 * 应用内提示的**呈现规约**（PRD-15-011 / PRD-15-020 / PRD-15-031）。纯函数，headless 可测。
 *
 * 三件事在这里定：走哪条通道、多久自动消散、同类怎么折叠与封顶。
 */

import type { NotificationEventDto } from "../../ipc/bindings";

/**
 * 通道优先级。注意 **inline 恒不会被选中**：后端路由表里带 inline 的两类（失败 / 需介入）
 * 同时带 toast 或 banner，故 inline 永远被更高优先级吃掉。这不是遗漏——PRD-15 的 inline
 * 语义是「就地提示」（队列项标红、字段级错误），由各功能在自己的界面上渲染，本就不该由
 * 通知宿主再浮一层。此处只保留两条真正会呈现的通道。
 */
const CHANNEL_PRIORITY = ["banner", "toast"] as const;

export type VisibleNotificationChannel = (typeof CHANNEL_PRIORITY)[number];

/**
 * 一条事件在应用内**落哪条通道**（`null` = 不入宿主，如纯 system/status 事件）。
 *
 * 返回单值而非数组：一条事件按优先级只会落一条通道，用长度恒为 0/1 的数组表达它，
 * 会让每个调用点都要写 `.includes(...)` 去问一个本可以直接比较的问题。
 */
export function visibleNotificationChannel(channels: string[]): VisibleNotificationChannel | null {
  return CHANNEL_PRIORITY.find((channel) => channels.includes(channel)) ?? null;
}

/**
 * toast 自动消散时长（ms）。分两档而非一刀切：**失败类要读的信息更多**（原因 + 下一步），
 * 8 秒读不完一条带操作建议的失败提示；完成/信息类看一眼即走。
 */
export const TOAST_DISMISS_MS = 6_000;
export const TOAST_FAILURE_DISMISS_MS = 10_000;

/**
 * 一条通知的自动消散时长；`null` = 持久，须用户处理。
 *
 * banner（系统健康 / 需用户介入）**刻意不自动消散**——PRD-15-020 的第一性是"防静默失败"：
 * 核心能力正在失效时把提示悄悄收走，等于回到了它要根治的那个问题。堆积由折叠与封顶
 * （[collapseNotificationStack]）治理，而不是靠让高危提示自己消失。
 */
export function autoDismissMs(item: Pick<NotificationEventDto, "class" | "channels">): number | null {
  if (visibleNotificationChannel(item.channels) !== "toast") {
    return null;
  }
  return item.class === "failure" ? TOAST_FAILURE_DISMISS_MS : TOAST_DISMISS_MS;
}

/**
 * 单条通道一次最多显示几条。超出的不是丢弃，而是折叠为一句「还有 N 条」并指向历史中心
 * ——屏幕被提示铺满时，用户既读不完也点不到底下的界面（真机反馈「通知堆积」）。
 */
export const MAX_VISIBLE_NOTIFICATIONS = 3;

/** 折叠后的一项：代表条 + 它折叠了几条同类。 */
export interface CollapsedNotification {
  /** 代表条 = 同类中**最新**的一条（参数与深链取最新，旧的已无行动价值）。 */
  item: NotificationEventDto;
  /** 同类条数（1 = 未折叠；>1 时界面显示计数徽标）。 */
  count: number;
}

/** 一条通道的呈现结果。 */
export interface NotificationStack {
  visible: CollapsedNotification[];
  /** 被折叠掉的**条数**（不是组数）——用户关心的是"还有几条没看"。 */
  overflow: number;
}

/**
 * 同类折叠 + 数量封顶（PRD-15-031「同类事件短时间窗内合并/折叠 + 计数」）。
 *
 * 折叠维度 = `i18n_key` + `target`：同一件事重复上报（如每 30s 复发一次的「麦克风无信号」）
 * 只占一行并累加计数，而不是每次都新增一张卡把屏幕堆满。后端 30s 去重窗只挡住窗口内的
 * 重复，跨窗复发仍会到达——**折叠必须在呈现层做**。
 *
 * 入参按时间倒序（最新在前，与 `useNotifications` 的累积顺序一致），返回同序。
 */
export function collapseNotificationStack(
  items: NotificationEventDto[],
  channel: VisibleNotificationChannel,
  maxVisible: number = MAX_VISIBLE_NOTIFICATIONS,
): NotificationStack {
  const groups = new Map<string, CollapsedNotification>();
  for (const item of items) {
    if (visibleNotificationChannel(item.channels) !== channel) {
      continue;
    }
    const key = `${item.i18n_key}:${item.target ?? ""}`;
    const existing = groups.get(key);
    if (existing) {
      existing.count += 1;
    } else {
      groups.set(key, { item, count: 1 });
    }
  }
  const all = [...groups.values()];
  const visible = all.slice(0, maxVisible);
  const overflow = all
    .slice(maxVisible)
    .reduce((sum, group) => sum + group.count, 0);
  return { visible, overflow };
}

/** 某条通道当前占用的全部通知 id（悬停暂停整栈计时用）。 */
export function stackNotificationIds(
  items: NotificationEventDto[],
  channel: VisibleNotificationChannel,
): string[] {
  return items
    .filter((item) => visibleNotificationChannel(item.channels) === channel)
    .map((item) => item.id);
}
