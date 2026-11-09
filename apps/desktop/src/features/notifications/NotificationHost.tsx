/**
 * 应用内提示宿主（PRD-15-011 / PRD-15-020 / PRD-15-031）。
 *
 * 两条通道各占一栈：**banner**（系统健康 / 需介入，顶部通栏、持久）与 **toast**（完成 /
 * 失败 / 信息，右下角、自动消散）。呈现规约（折叠、封顶、TTL）全在纯函数
 * [notification-routing]，本组件只渲染。
 *
 * 「堆积」的两道闸门（真机反馈）：同类**折叠成一行 + 计数**，超出上限的收进一句
 * 「还有 N 条」并指向历史中心——绝不让提示铺满屏幕、盖住底下的界面。
 */

import { useTranslation } from "react-i18next";
import { IconButton } from "../../components/ui/IconButton";
import { CloseIcon } from "@lunarca/editor-core/Icon";
import type { NotificationEventDto } from "../../ipc/bindings";
import {
  collapseNotificationStack,
  stackNotificationIds,
  type VisibleNotificationChannel,
} from "./notification-routing";
import { resolveNotificationBaseKey } from "./notification-i18n";
import { useNotifications } from "./use-notifications";
import { goTo } from "../../lib/navigate";

/** 通知历史（动态·通知页签）的深链——「查看历史」与溢出条共用。 */
const HISTORY_ROUTE = "notifications://";

function NotificationItem({
  item,
  count,
  channel,
  onDismiss,
}: {
  item: NotificationEventDto;
  /** 该条折叠了几条同类（>1 时显示计数徽标）。 */
  count: number;
  channel: VisibleNotificationChannel;
  onDismiss: (id: string) => void;
}) {
  const { t, i18n } = useTranslation("notifications");
  const baseKey = resolveNotificationBaseKey(item.i18n_key, i18n);
  // 事件参数（如模型名）由后端脱敏后随 DTO 下发，文案占位符 {{param}} 在此插值。
  const title = t(`${baseKey}.title`, item.params);
  const body = t(`${baseKey}.body`, item.params);
  const actionLabel = i18n.exists(`${baseKey}.action`) ? t(`${baseKey}.action`) : null;

  return (
    <article
      className={`luna-notice-card luna-notification luna-notification--${channel}`}
      data-severity={item.severity}
      aria-live={item.severity === "critical" ? "assertive" : "polite"}
    >
      <header className="luna-notice-card__header">
        <strong>{title}</strong>
        {count > 1 && (
          <span
            className="luna-notification__count"
            title={t("repeated", { count })}
            aria-label={t("repeated", { count })}
          >
            ×{count}
          </span>
        )}
        {/* 关闭件统一为 `CloseIcon` + 幽灵图标钮（全应用此前有 ✕ / &times; / × / 文字四种）。 */}
        <IconButton
          label={t("action.dismiss")}
          icon={<CloseIcon size={12} />}
          onClick={() => {
            onDismiss(item.id);
          }}
        />
      </header>
      <p className="luna-notice-card__body">{body}</p>
      <footer className="luna-notification__actions">
        {item.target && actionLabel ? (
          <button
            type="button"
            className="luna-notice-card__action luna-notice-card__action--primary"
            onClick={() => goTo(item.target as string)}
          >
            {actionLabel}
          </button>
        ) : null}
        <button
          type="button"
          className="luna-notice-card__action"
          onClick={() => goTo(HISTORY_ROUTE)}
        >
          {t("action.history")}
        </button>
      </footer>
    </article>
  );
}

function NotificationLane({
  items,
  channel,
  onDismiss,
  onPause,
  onResume,
}: {
  items: NotificationEventDto[];
  channel: VisibleNotificationChannel;
  onDismiss: (id: string) => void;
  onPause: (ids: string[]) => void;
  onResume: (ids: string[]) => void;
}) {
  const { t } = useTranslation("notifications");
  const stack = collapseNotificationStack(items, channel);
  if (stack.visible.length === 0) {
    return null;
  }
  const laneIds = stackNotificationIds(items, channel);
  return (
    <section
      className={`luna-notification-host luna-notification-host--${channel}`}
      aria-label={t("hostLabel")}
      // 悬停/聚焦整栈即暂停消散（读到一半不该消失）；移开按剩余时长续时。
      onMouseEnter={() => onPause(laneIds)}
      onMouseLeave={() => onResume(laneIds)}
      onFocusCapture={() => onPause(laneIds)}
      onBlurCapture={() => onResume(laneIds)}
    >
      {stack.visible.map(({ item, count }) => (
        <NotificationItem
          key={`${channel}-${item.id}`}
          item={item}
          count={count}
          channel={channel}
          onDismiss={onDismiss}
        />
      ))}
      {stack.overflow > 0 && (
        <button
          type="button"
          className="luna-notification-host__overflow"
          onClick={() => goTo(HISTORY_ROUTE)}
        >
          {t("overflow", { count: stack.overflow })}
        </button>
      )}
    </section>
  );
}

export function NotificationHost() {
  const { items, dismiss, pauseDismiss, resumeDismiss } = useNotifications();
  if (items.length === 0) {
    return null;
  }
  return (
    <>
      <NotificationLane
        items={items}
        channel="banner"
        onDismiss={dismiss}
        onPause={pauseDismiss}
        onResume={resumeDismiss}
      />
      <NotificationLane
        items={items}
        channel="toast"
        onDismiss={dismiss}
        onPause={pauseDismiss}
        onResume={resumeDismiss}
      />
    </>
  );
}
