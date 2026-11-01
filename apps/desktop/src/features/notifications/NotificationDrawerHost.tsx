/**
 * 应用级通知抽屉宿主（PRD-15-064 / PRD-07-056 / ADR-087 ②）。
 *
 * 「有什么值得知道」这条状态路径的**唯一入口宿主**：状态栏铃铛与 `notifications://` 深链
 * 打开的是同一个它。抽屉覆盖在当前工作上下文之上——**不切换工作区目的地、不依赖任何一级
 * 导航页面**，故它挂在 `App` 里与 `NotificationHost` 同层，而不是长在工作区壳的某个页面上。
 *
 * **路由归它自己听**（与 `GlossaryQuickAdd` 的「事件 + 唯一宿主」同一形态）：
 * `notifications://` 不再经工作区壳的 `onNavigate` 分发到某个目的地——那正是 ADR-087 ②
 * 要拆掉的东西（数据同源不等于入口同页）。工作区壳里那条分支随本批删除，此后这个前缀
 * 在全仓只有这一个消费者。
 *
 * 三条关闭手势（PRD-15-064）：Esc / 关闭按钮 / 再点铃铛。第三条**由遮罩承担**——抽屉是
 * 带焦点圈定的模态覆盖层，铃铛此刻在遮罩之下，点它落到的是遮罩，于是"再点铃铛就关掉了"
 * 恰好成立，且焦点随即归还给铃铛本身。这与全仓十处对话框是**同一个形制**
 *（`.luna-overlay` + `role="presentation"` + 点遮罩关闭，见 `components/dialog.css`）。
 *
 * **路由的语义是"打开"，不是 toggle**：初版把它写成 toggle 来兑现"再点铃铛"，但那颗
 * 铃铛在抽屉开着时根本点不到（遮罩盖住），于是那条分支在真机上一次都不会被执行——一个
 * 既不可达、又与既有关闭手势分叉的设计。收敛后这个前缀只回答一件事：**把历史摆到用户
 * 面前**（辅助窗口经 `open_workspace_route` 送来的同一请求也该如此，而不是把已经开着的
 * 抽屉关掉）。详情里的**业务深链**是另一个前缀，到达即关闭抽屉并前往目标。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { useStatusVisitMetrics } from "../../hooks/use-status-visit-metrics";
import { NotificationHistory } from "./NotificationHistory";
import { NAVIGATE_EVENT } from "../../lib/navigate";

/** 打开/切换通知抽屉的深链前缀（铃铛、通知卡「查看历史」、溢出条共用）。 */
export const NOTIFICATION_DRAWER_ROUTE = "notifications://";

/**
 * 一次 `luna:navigate` 之后抽屉该不该开着——纯函数，便于无头验证。
 *
 * - 本前缀 → 开（无论此前是开是关：它表达的是"把历史摆到我面前"）；
 * - 其它任何路由 → 关（详情里的业务深链点完就该走，不该把抽屉留在原地挡着目标）。
 */
export function drawerOpenForRoute(route: string): boolean {
  return route.startsWith(NOTIFICATION_DRAWER_ROUTE);
}

export function NotificationDrawerHost() {
  const { t } = useTranslation("notifications");
  const [open, setOpen] = useState(false);
  /**
   * 详情模态是否开着——开着时抽屉**让出 Esc**。
   *
   * 两层 `useDialogBehavior` 的 Esc 监听都挂在 `document` 上，同一元素上的监听器不会
   * 被 `stopPropagation` 拦下（那要 `stopImmediatePropagation`）。不让出的话按一次 Esc
   * 会同时关掉详情**和**抽屉——用户以为自己只是退出了一条通知的详情。
   */
  const [detailOpen, setDetailOpen] = useState(false);

  /**
   * 「有什么值得知道」这条状态路径的观测（PRD-07-070 / TDD-12 §2.1）。
   *
   * **宿主是它，不是铃铛**：铃铛只是三个打开手势之一（还有 `notifications://` 深链与
   * 辅助窗口的同一请求），在铃铛上记会漏掉另外两条；而"到访"的终点是抽屉关上那一刻，
   * 铃铛根本看不到。这也正是 §2.1「只在唯一导航/动作宿主发送」要的落点。
   */
  const notificationVisit = useStatusVisitMetrics("notifications", open);

  useEffect(() => {
    const onNavigate = (event: Event) => {
      const route = event instanceof CustomEvent ? String(event.detail) : "";
      setOpen(drawerOpenForRoute(route));
    };
    window.addEventListener(NAVIGATE_EVENT, onNavigate);
    return () => {
      window.removeEventListener(NAVIGATE_EVENT, onNavigate);
    };
  }, []);

  // 关闭时详情态一并复位：下次打开抽屉不该继承上一次的"详情开着"。
  useEffect(() => {
    if (!open) {
      setDetailOpen(false);
    }
  }, [open]);

  /**
   * 让路写在 `onClose` 里，**不能写成 `closeOnEscape: !detailOpen`**。
   *
   * `closeOnEscape` 在 `useDialogBehavior` 的依赖数组里：它一变，整个效果重建，而那个
   * 效果的**清理函数正是焦点归还**——于是"打开一条通知的详情"会先把焦点还给铃铛、再被
   * 重建的焦点圈定拉回抽屉的关闭键，用户刚打开的详情当场失焦。`onClose` 走的是
   * `onCloseRef`（每次渲染更新、不进依赖），故这条判据每帧都是新鲜的而效果一次都不重建。
   *
   * 监听顺序天然正确：抽屉先注册、先执行，读到详情开着就什么都不做；随后详情自己那条
   * 监听把详情关掉。一次 Esc 只退一层。
   */
  const dialogRef = useDialogBehavior<HTMLDivElement>({
    onClose: () => {
      if (detailOpen) {
        return;
      }
      setOpen(false);
    },
    enabled: open,
  });

  if (!open) {
    return null;
  }

  return (
    <div
      className="luna-overlay luna-overlay--drawer"
      role="presentation"
      // 判据是 `target === currentTarget` 而不是"卡片里 stopPropagation"：通知**详情**
      // 是叠在抽屉之上的第二层模态，而它在 DOM 上是抽屉的后代——点详情的遮罩会冒泡到这里，
      // 于是"关掉一条通知的详情"会连抽屉一起关掉。只认落在遮罩自己身上的那一下即可。
      onClick={(event) => {
        if (event.target === event.currentTarget) {
          setOpen(false);
        }
      }}
    >
      <div
        ref={dialogRef}
        className="luna-dialog luna-dialog--panel luna-notification-drawer"
        role="dialog"
        aria-modal="true"
        // 可访问名取历史面板自己的标题：抽屉不再画第二个标题条
        //（同一事实在同一屏只呈现一次，PRD-07-065）。
        aria-label={t("history.title")}
        tabIndex={-1}
      >
        {/* 关闭键**并入历史面板自己的页头**，不再单占一条工具条。此前抽屉顶上是一颗
            孤零零浮在标题之上的 ✕，下面才是面板的标题行——同一个面板长了两个头
            （真机 2026-09-09「元件简单粗暴地罗列」）。 */}
        <div className="luna-notification-drawer__body" {...notificationVisit.interactionProps}>
          <NotificationHistory
            onClose={() => setOpen(false)}
            onDetailOpenChange={(next) => {
              // 打开一条通知的详情是这条路径上最实的一次"用起来了"——它也可能来自键盘
              // 或深链，不一定经过上面那两个捕获监听。
              if (next) {
                notificationVisit.markInteracted();
              }
              setDetailOpen(next);
            }}
          />
        </div>
      </div>
    </div>
  );
}
