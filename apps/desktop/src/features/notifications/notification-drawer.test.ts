/// <reference types="vite/client" />
// 应用级通知抽屉（PRD-15-064 / PRD-15-065 / ADR-087 ②）：开合语义 + 宿主接线。
//
// 开合是纯函数（`nextDrawerOpen`），可直接验证三条关闭手势；其余（Esc / 焦点圈定 /
// 焦点归还 / 复用同一份历史与未读口径）是**接线**，仓库没有 jsdom，故按
// `draft-badge-events.test.ts` 的先例做源码断言——这里要拦的恰好都是源码层面的遗漏：
// 少挂一个 hook、少传一个回调、或者在抽屉里另抄一份历史列表，三者都不会让任何既有
// 用例变红，而用户看到的是"Esc 关不掉""焦点跑到抽屉外面""两处未读数对不上"。

import { describe, expect, it } from "vitest";
import { NOTIFICATION_DRAWER_ROUTE, drawerOpenForRoute } from "./NotificationDrawerHost";

const sources = import.meta.glob(
  "./{NotificationDrawerHost.tsx,NotificationHistory.tsx,NotificationHost.tsx,use-notification-unread.ts}",
  { eager: true, import: "default", query: "?raw" },
) as Record<string, string>;

const DRAWER = sources["./NotificationDrawerHost.tsx"] ?? "";
const HISTORY = sources["./NotificationHistory.tsx"] ?? "";
const TOAST_HOST = sources["./NotificationHost.tsx"] ?? "";
const UNREAD = sources["./use-notification-unread.ts"] ?? "";

/**
 * 抽屉的**代码**（剥掉注释）。
 *
 * 有两条断言问的是"这个写法有没有回来"，而注释里恰好逐字解释了为什么不能那么写——
 * 拿整份文件断言会在解释上误红，然后下一个人会把那段解释删掉以让门禁变绿。
 */
const stripComments = (source: string) =>
  source
    .replace(/\/\*[\s\S]*?\*\//g, "\n")
    .split("\n")
    .filter((line) => !/^\s*(\/\/|\*)/.test(line))
    .join("\n");

const DRAWER_CODE = stripComments(DRAWER);

/**
 * 历史面板的**代码**（剥掉注释）。
 *
 * 判"某个调用在不在"必须看代码，不能看全文：`t("history.read")` 恰好出现在一段解释
 * "为什么不再逐行写已读"的注释里，全文断言会被那段解释喂饱而**恒绿**——门禁一旦能被
 * 散文满足，它就不再是门禁了。
 */
const HISTORY_CODE = stripComments(HISTORY);

const apps = import.meta.glob("../../app/{App.tsx,WorkspaceApp.tsx,StatusBarIndicators.tsx}", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const APP = apps["../../app/App.tsx"] ?? "";
const WORKSPACE_APP = apps["../../app/WorkspaceApp.tsx"] ?? "";
/** 底栏铃铛：自带未读订阅的叶子（工作区根只摆放它，不持有未读数）。 */
const STATUS_BAR = apps["../../app/StatusBarIndicators.tsx"] ?? "";

describe("抽屉开合语义（PRD-15-064）", () => {
  it("`notifications://` 是唯一打开它的深链", () => {
    expect(NOTIFICATION_DRAWER_ROUTE).toBe("notifications://");
    expect(drawerOpenForRoute("notifications://")).toBe(true);
  });

  it("这个前缀的语义是「把历史摆到我面前」，不是 toggle", () => {
    // 初版写成 toggle 是为了兑现「再点铃铛可关闭」，但抽屉开着时铃铛在遮罩之下、
    // 根本点不到，那条分支在真机上一次都不会执行。更糟的是辅助窗口经
    // `open_workspace_route` 送来的同一请求会把已经开着的抽屉**关掉**。
    // 关闭手势归 Esc / 关闭键 / 点遮罩（下一个 describe 锁的就是它）。
    expect(drawerOpenForRoute("notifications://")).toBe(true);
  });

  it("业务深链到达即关闭并让路——抽屉不该留在原地挡着用户刚要去的地方", () => {
    for (const route of [
      "notes://Notes/示例.md",
      "settings://ai_services/usage",
      "queue://",
      "graph://Notes/%E7%A4%BA%E4%BE%8B.md",
    ]) {
      expect(drawerOpenForRoute(route)).toBe(false);
    }
  });

  it("空 detail（非 CustomEvent 的杂音）不打开抽屉", () => {
    expect(drawerOpenForRoute("")).toBe(false);
  });
});

describe("抽屉宿主接线（模态行为 / 单一实现 / 挂载层）", () => {
  it("扫描面非空", () => {
    for (const src of [DRAWER, HISTORY, TOAST_HOST, UNREAD, APP, WORKSPACE_APP, STATUS_BAR]) {
      expect(src.length).toBeGreaterThan(200);
    }
  });

  it("Esc / 焦点圈定 / 焦点归还走共享 hook，不手写第二份", () => {
    // 三件事必须一起给：仓库里"各写各的 Esc、零个焦点圈定"的历史见 `use-dialog-behavior`
    // 的抬头。抽屉是模态覆盖层，缺任一件它就不成立。
    expect(DRAWER).toContain("useDialogBehavior");
    expect(DRAWER_CODE).toContain("setOpen(false)");
    expect(DRAWER).toContain('role="dialog"');
    expect(DRAWER).toContain('aria-modal="true"');
  });

  it("详情模态开着时抽屉让出 Esc——否则按一次会连抽屉一起关掉", () => {
    // 两层 `useDialogBehavior` 的 keydown 都挂在 `document` 上，同一元素上的监听不会被
    // `stopPropagation` 拦下。让路的事实由历史面板的 `onDetailOpenChange` 回调提供。
    expect(DRAWER_CODE).toContain("if (detailOpen) {");
    // 断的是**事实**而不是那一行字面量：状态面到访观测在同一个回调里加了一笔
    // `markInteracted()`，于是它从 `={setDetailOpen}` 变成了一个小函数。抽屉仍然从这条
    // 回调拿到详情态——那才是这条用例要守的东西。
    expect(DRAWER_CODE).toContain("onDetailOpenChange={");
    expect(DRAWER_CODE).toContain("setDetailOpen(next)");
    expect(HISTORY).toContain("onDetailOpenChange?.(true)");
    expect(HISTORY).toContain("onDetailOpenChange?.(false)");
  });

  it("让路不得写成 `closeOnEscape` —— 它在依赖数组里，一变就把焦点还回去", () => {
    // `useDialogBehavior` 的清理函数就是焦点归还。把每帧都可能变的判据放进它的依赖，
    // 等于"打开详情 → 效果重建 → 焦点先还给铃铛、再被拉回抽屉关闭键"，详情当场失焦。
    // 这一条是写实现时自己踩到的，留下来免得下一次被"看起来更直白"的写法换回去。
    expect(DRAWER_CODE).not.toContain("closeOnEscape");
  });

  it("点遮罩关闭，且只认落在遮罩自己身上的那一下", () => {
    // ① 全仓十处对话框都是 `.luna-overlay` + `role="presentation"` + 点遮罩关闭；抽屉
    //    此前是唯一的例外，于是 PRD-15-064 明列的「再点铃铛」在真机上做不到——那颗铃铛
    //    在遮罩之下，点它落到的就是这层遮罩。
    // ② 判据必须是 `target === currentTarget`：通知**详情**是叠在抽屉之上的第二层模态，
    //    DOM 上却是抽屉的后代，它的遮罩点击会冒泡到这里——无条件关闭会让"关掉一条通知的
    //    详情"连抽屉一起关掉。
    const overlay = DRAWER_CODE.split('className="luna-overlay luna-overlay--drawer"')[1]?.split(
      "<div",
    )[0];
    expect(overlay, "抽屉遮罩的开标签切不出来了").toBeDefined();
    expect(overlay).toContain('role="presentation"');
    expect(overlay).toContain("if (event.target === event.currentTarget)");
  });

  it("触发器声明它会打开一扇对话框（读屏在按下之前就该知道）", () => {
    expect(STATUS_BAR).toContain('aria-haspopup="dialog"');
  });

  it("抽屉有可读标题，且不再画第二个标题条", () => {
    // 可访问名取历史面板自己的标题：同一事实在同一屏只呈现一次（PRD-07-065）。读屏
    // 打开它时必须听见"通知历史"，而不是一个没有名字的对话框。
    expect(DRAWER).toContain('aria-label={t("history.title")}');
  });

  it("未读是**文字**，不只是一个颜色（PRD-00 §7 / PRD-15-065）", () => {
    // 只靠标题加粗或一个色点表达未读，对读屏用户等于没有这条信息，对色觉障碍用户同理。
    expect(HISTORY_CODE).toContain('t("history.unread")');
    // 「已读」不再逐行复述：那是默认状态，每行写一次只是噪声，而"没有未读标记"本身就是
    // 读屏能读出的同一件事（邮件类产品同此惯例）。要守的是**未读有文字**，不是"两个词
    // 都得出现"。
    expect(HISTORY_CODE).not.toContain('t("history.read")');
  });

  it("关闭按钮有可读名（图标钮的名字必填，不靠 title 兜底）", () => {
    // 关闭键住在**历史面板自己的页头**里：抽屉此前在标题行之上另起一条工具条只放一颗
    // ✕，同一个面板长了两个头（真机 2026-09-09）。判据跟着搬家，要求没有放松。
    expect(HISTORY).toContain("<IconButton");
    expect(HISTORY).toContain('label={t("drawer.close")}');
    expect(DRAWER).toContain("onClose={() => setOpen(false)}");
  });

  it("抽屉复用同一份历史实现与同一个未读口径，不另抄一份", () => {
    // "同源不同页"（ADR-087 ②）的前提是**实现只有一份**：抽屉里再抄一份列表，
    // 标已读/清空就会有两套刷新路径，未读角标与面板迟早对不上（PRD-15-062）。
    expect(DRAWER).toContain("<NotificationHistory");
    expect(DRAWER).not.toContain("notificationHistory(");
    expect(DRAWER).not.toContain("notificationUnreadCount");
    // 未读计数仍只有一处取数，并同时听后端事件与前端标已读广播。
    expect(UNREAD).toContain("commands.notificationUnreadCount()");
    expect(UNREAD).toContain("events.notificationEvent.listen");
    expect(UNREAD).toContain("NOTIFICATIONS_CHANGED_EVENT");
  });

  it("挂在 App 层与工作区同级——它不是工作区的一页，也不随切库重挂载", () => {
    expect(APP).toContain("<NotificationDrawerHost />");
    // `key={vaultEpoch}` 只给工作区：抽屉里的历史是**设备级 ∪ 当前 Vault** 的合并投影，
    // 跟着切库整体重挂载没有意义（PRD-15-060）。
    expect(APP).toMatch(/<WorkspaceApp\s+key=\{vaultEpoch\}/);
    expect(APP).not.toContain("<NotificationDrawerHost key=");
  });

  it("抽屉**独占**这个前缀：工作区壳里不再有它的分支", () => {
    // 迁移前 `notifications://` 把用户送进「动态」目的地的「通知」页签——ADR-087 ② 拆掉的
    // 正是那个容器。留着旧分支就会有两个宿主同时响应同一次导航（一个开抽屉、一个切页面）。
    const handler =
      WORKSPACE_APP.split("const onNavigate = (event: Event) => {")[1]?.split(
        "const onRecordingArchived",
      )[0] ?? "";
    expect(handler.length).toBeGreaterThan(200);
    expect(handler).not.toContain('route.startsWith("notifications://")');
    // 事件名走共享常量（`lib/navigate.ts`）——两边各写一遍字面量正是"改了发的那头、
    // 忘了收的那头"的温床。
    expect(DRAWER_CODE).toContain("window.addEventListener(NAVIGATE_EVENT, onNavigate)");
  });

  it("铃铛与「查看历史」发的是同一个前缀 —— 一个入口，一个宿主", () => {
    expect(STATUS_BAR).toContain('goTo("notifications://")');
    expect(TOAST_HOST).toContain('const HISTORY_ROUTE = "notifications://"');
  });

  it("页头的总数与未读数来自后端，不数已加载的那一页（PRD-15-062）", () => {
    // 一页 100 条、表里 500 条时，`items.length` 会让页头写着「100 条」——而那正是用户
    // 判断"要不要清"的依据；未读同理，只数这一页会与状态栏角标对不上。
    expect(HISTORY_CODE).toContain("setTotals");
    expect(HISTORY_CODE).toContain("{totals.total}");
    expect(HISTORY_CODE).toContain("const unreadCount = totals.unread");
    expect(HISTORY_CODE).not.toContain("items.filter((item) => !item.read)");
  });

  it("标单行已读不整份重取——那会把用户翻到的页数一起丢掉", () => {
    expect(HISTORY_CODE).toContain("const changed = await commands.notificationMarkRead");
    // 按后端实际改动条数扣减；直接减 1 会在"本来就已读"的条目上把角标扣穿。
    expect(HISTORY_CODE).toContain("current.unread - changed");
  });

  it("状态栏铃铛的角标读的是同一个未读 hook（PRD-15-062）", () => {
    expect(STATUS_BAR).toContain("const unread = useNotificationUnread();");
    expect(STATUS_BAR).toContain("data-unread={unread > 0}");
    expect(WORKSPACE_APP).toContain("<StatusBarNotifications />");
  });
});
