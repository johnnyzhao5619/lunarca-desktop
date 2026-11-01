// 通知历史的**筛选口径**（纯函数，PRD-15-060 / PRD-15-065）。Headless 可测。
//
// 此前这一页只有一个搜索框：一条筛选都没有。而同行（Slack Activity / Linear Inbox /
// GitHub Notifications）无一例外先给「未读」这一档——因为历史中心真正的日常问题不是
// 「那条叫什么」，而是**「还有什么没处理」**。搜索回答的是前者，它替代不了后者。

/** 后端 `NotificationClass::as_str()` 的全集（呈现顺序即本清单顺序）。 */
export const NOTIFICATION_CLASSES = [
  "system_health",
  "user_action",
  "failure",
  "completion",
  "info",
] as const;

export type NotificationClassId = (typeof NOTIFICATION_CLASSES)[number];

/** 当前的筛选条件。`classes` 为空 = 不按类别筛（**不是**"一个都不要"）。 */
export interface NotificationFilter {
  unreadOnly: boolean;
  classes: readonly string[];
}

/** 什么都没筛的初始条件。 */
export const NO_NOTIFICATION_FILTER: NotificationFilter = { unreadOnly: false, classes: [] };

/** 筛选面向的最小形状。 */
interface Filterable {
  class: string;
  read: boolean;
}

/**
 * 按条件筛。两条判据是**与**关系：勾了「未读」又选了「失败」，要的是"没处理的失败"。
 *
 * `classes` 为空表示**不按类别筛**：把它解释成"一个都不要"会让用户在取消最后一个
 * 勾选之后看到一片空白，而他做的动作叫"不再按类别筛"。
 */
export function filterNotifications<T extends Filterable>(
  items: readonly T[],
  filter: NotificationFilter,
): T[] {
  return items.filter((item) => {
    if (filter.unreadOnly && item.read) {
      return false;
    }
    return filter.classes.length === 0 || filter.classes.includes(item.class);
  });
}

/** 切换一个类别的勾选态（不可变；供菜单项直接调用）。 */
export function toggleNotificationClass(
  filter: NotificationFilter,
  value: string,
): NotificationFilter {
  const classes = filter.classes.includes(value)
    ? filter.classes.filter((item) => item !== value)
    : [...filter.classes, value];
  return { ...filter, classes };
}

/** 此刻是否处于「筛过」的状态——页头据此说清"这不是全部"。 */
export function isNotificationFilterActive(filter: NotificationFilter): boolean {
  return filter.unreadOnly || filter.classes.length > 0;
}
