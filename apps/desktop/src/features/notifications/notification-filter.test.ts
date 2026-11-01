import { describe, expect, it } from "vitest";
import {
  NOTIFICATION_CLASSES,
  NO_NOTIFICATION_FILTER,
  filterNotifications,
  isNotificationFilterActive,
  toggleNotificationClass,
} from "./notification-filter";

const items = [
  { id: "1", class: "failure", read: false },
  { id: "2", class: "failure", read: true },
  { id: "3", class: "completion", read: false },
  { id: "4", class: "system_health", read: true },
];
const ids = (list: typeof items) => list.map((item) => item.id);

describe("filterNotifications", () => {
  it("什么都没筛时原样返回（顺序不动）", () => {
    expect(ids(filterNotifications(items, NO_NOTIFICATION_FILTER))).toEqual(["1", "2", "3", "4"]);
  });

  it("只看未读", () => {
    expect(ids(filterNotifications(items, { unreadOnly: true, classes: [] }))).toEqual(["1", "3"]);
  });

  it("两条判据是与关系：没处理的失败", () => {
    expect(ids(filterNotifications(items, { unreadOnly: true, classes: ["failure"] }))).toEqual([
      "1",
    ]);
  });

  // 取消最后一个勾选时用户做的动作叫「不再按类别筛」，不是「一个都不要」。
  it("类别为空 = 不按类别筛，不是一条都不给", () => {
    const cleared = toggleNotificationClass({ unreadOnly: false, classes: ["failure"] }, "failure");
    expect(cleared.classes).toEqual([]);
    expect(ids(filterNotifications(items, cleared))).toHaveLength(items.length);
  });

  it("多选类别取并集", () => {
    expect(
      ids(filterNotifications(items, { unreadOnly: false, classes: ["completion", "system_health"] })),
    ).toEqual(["3", "4"]);
  });
});

describe("toggleNotificationClass", () => {
  it("勾上与取消都不改原对象", () => {
    const base = NO_NOTIFICATION_FILTER;
    const next = toggleNotificationClass(base, "info");
    expect(next.classes).toEqual(["info"]);
    expect(base.classes).toEqual([]);
  });
});

describe("isNotificationFilterActive", () => {
  it("任一条件在场即为已筛", () => {
    expect(isNotificationFilterActive(NO_NOTIFICATION_FILTER)).toBe(false);
    expect(isNotificationFilterActive({ unreadOnly: true, classes: [] })).toBe(true);
    expect(isNotificationFilterActive({ unreadOnly: false, classes: ["info"] })).toBe(true);
  });
});

describe("NOTIFICATION_CLASSES", () => {
  // 后端 `NotificationClass::as_str()` 的全集；漏一档 = 那一类通知在筛选菜单里不存在。
  it("覆盖后端五类且不重复", () => {
    expect([...NOTIFICATION_CLASSES].sort()).toEqual(
      ["completion", "failure", "info", "system_health", "user_action"].sort(),
    );
    expect(new Set(NOTIFICATION_CLASSES).size).toBe(NOTIFICATION_CLASSES.length);
  });
});
