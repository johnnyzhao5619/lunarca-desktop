/**
 * 「去某个地方」——全应用**唯一**的一处（PRD-07-035 / TDD-07 §2.4）。
 *
 * 此前这件事有两条路，而选哪条取决于**你在哪扇窗**：主窗口派发 `luna:navigate`
 * CustomEvent（37 处各写一遍），辅助窗口（HUD / 面板 / 独立笔记窗 / 便签）走
 * `open_workspace_route` 唤起主窗（10 处）。每个调用点都得自己知道该走哪条，而
 * `VaultEditor` 甚至专门有一个 prop（`onOpenInGraph`）存在的唯一理由就是让调用方替它
 * 选通道。
 *
 * 那样写有两个具体的坏处，都不会有任何东西变红：
 * ① 事件名这个字符串被手写了 37 遍——打错一个字母，那个按钮从此点了没反应；
 * ② 一个组件被拿到辅助窗里复用时，它的导航会静默失效（那扇窗没有监听器）。
 *
 * 故通道选择收在这里：`goTo` 自己问"这扇窗里有人管导航吗"。调用方只说去哪儿。
 */

import { commands } from "../ipc/bindings";

/**
 * 应用内导航事件名。
 *
 * **监听方也必须用这个常量**：两边各写一遍字面量，就是"改了发的那头、忘了收的那头"
 * 这类 bug 的温床。
 */
export const NAVIGATE_EVENT = "luna:navigate";

/**
 * 去这个路由（`settings://…` / `notes://…` / `graph://…` 等）。
 *
 * 就地问一声「这扇窗里有人管导航吗」：工作区壳的处理器收下之后调 `preventDefault()`
 * 表示"我接了"；没人接（浮窗、便签、独立笔记窗，以及引导期还没挂上工作区的主窗）就经
 * Rust 唤起主窗转发——后者会把主窗取消最小化、显示并聚焦，那正是从一扇浮窗点
 * 「打开模型设置」时该发生的事。
 *
 * **判据是"有没有人接"，不是"我是不是主窗"**（2026-09-09 真机修订）：初版按
 * `getCurrentWebviewWindow().label === "main"` 猜，而那个调用一旦取不到窗口身份就只能
 * 猜一个默认值——猜"是主窗"时浮窗里的导航**静默消失**（便签上的「在 Lunarca 中打开」
 * 点了没反应），猜"不是"时主窗会把自己的导航绕一圈 Rust 再发回来，还可能与转发处理器
 * 转成死循环。而"有没有人接"是**当场可观测的事实**，不需要猜。
 *
 * **辅助窗那条路带白名单**（`workspace_route_allowed`）：不在册的 scheme 会被 Rust
 * 如实拒绝而不是静默吞掉。加新 scheme 时两处都要过——那是有意的，跨窗口能到达的目的地
 * 是一份需要显式维护的清单。
 */
export function goTo(route: string): void {
  // `cancelable` 才使 `preventDefault()` 有效；`dispatchEvent` 返回 false 即有人接了。
  const taken = !window.dispatchEvent(
    new CustomEvent(NAVIGATE_EVENT, { detail: route, cancelable: true }),
  );
  if (taken) {
    return;
  }
  void commands.openWorkspaceRoute(route);
}

/**
 * 「我接了这次导航」——导航处理器收下之后必须调它。
 *
 * 收成一句话而不是让各处自己写 `event.preventDefault()`：那行代码在这里的意思不是
 * "阻止默认行为"，而是"别再往 Rust 兜一圈了"，写成它自己的名字才读得懂。
 */
export function navigationHandled(event: Event): void {
  event.preventDefault();
}

/** 打开一篇笔记（最常见的那一个目的地，省得每处各拼一次 `notes://`）。 */
export function openNote(path: string): void {
  goTo(`notes://${path}`);
}
