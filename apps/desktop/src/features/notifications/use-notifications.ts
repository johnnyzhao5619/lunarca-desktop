import { useCallback, useEffect, useRef, useState } from "react";
import { commands, events, type NotificationEventDto } from "../../ipc/bindings";
import { autoDismissMs } from "./notification-routing";
import { deliverSystemNotification } from "./notification-system";

const MAX_NOTIFICATIONS = 20;

/** 一条待消散通知的计时状态（暂停 = 清掉 timer 并记住剩余时长，恢复时按剩余重新计时）。 */
interface DismissTimer {
  timer: number | null;
  /** 计时恢复后的到期时刻（`performance.now()` 口径）；暂停时为 null。 */
  deadline: number | null;
  /** 暂停时剩余的毫秒数；运行中为 null。 */
  remaining: number | null;
}

export function useNotifications() {
  const [items, setItems] = useState<NotificationEventDto[]>([]);
  const [unreadCount, setUnreadCount] = useState(0);
  // 自动消散计时器（PRD-15-011 瞬时语义）；卸载时统一清理防泄漏。
  const timersRef = useRef<Map<string, DismissTimer>>(new Map());

  const refreshUnreadCount = useCallback(() => {
    void commands.notificationUnreadCount().then(setUnreadCount);
  }, []);

  const drop = useCallback((id: string) => {
    setItems((current) => current.filter((item) => item.id !== id));
  }, []);

  /** 为一条通知起（或重起）消散计时。 */
  const arm = useCallback(
    (id: string, ttl: number) => {
      const entry = timersRef.current.get(id) ?? { timer: null, deadline: null, remaining: null };
      entry.timer = window.setTimeout(() => {
        timersRef.current.delete(id);
        drop(id);
      }, ttl);
      entry.deadline = performance.now() + ttl;
      entry.remaining = null;
      timersRef.current.set(id, entry);
    },
    [drop],
  );

  useEffect(() => {
    const timers = timersRef.current;
    const unlisten = events.notificationEvent.listen((event) => {
      void (async () => {
        // 系统通道（PRD-15-012）：Rust 按在场态判定带不带 `system`；这里负责真的发出去。
        // 发不出去（用户在系统设置里关了、或平台不支持）= 降级为应用内 toast（PRD-15-001），
        // 绝不静默丢失。
        const payload = await deliverSystemNotification(event.payload);
        setItems((current) => [payload, ...current].slice(0, MAX_NOTIFICATIONS));
        refreshUnreadCount();
        // toast = 瞬时非阻断（PRD-15-011）：到时自动收起（历史中心恒可回看）；
        // banner 持久，须用户处理（PRD-15-020 防静默失败）。
        const ttl = autoDismissMs(payload);
        if (ttl !== null) {
          arm(payload.id, ttl);
        }
      })();
    });
    refreshUnreadCount();

    return () => {
      for (const entry of timers.values()) {
        if (entry.timer !== null) {
          window.clearTimeout(entry.timer);
        }
      }
      timers.clear();
      void unlisten.then((dispose) => {
        dispose();
      });
    };
  }, [arm, refreshUnreadCount]);

  const dismiss = useCallback(
    (id: string) => {
      const entry = timersRef.current.get(id);
      if (entry?.timer != null) {
        window.clearTimeout(entry.timer);
      }
      timersRef.current.delete(id);
      drop(id);
    },
    [drop],
  );

  /**
   * 悬停/聚焦时暂停消散计时（标准 toast 行为）：正读到一半的提示不该在眼前消失，
   * 尤其失败类还带着「下一步怎么办」。
   */
  const pauseDismiss = useCallback((ids: string[]) => {
    for (const id of ids) {
      const entry = timersRef.current.get(id);
      if (!entry || entry.timer === null || entry.deadline === null) {
        continue;
      }
      window.clearTimeout(entry.timer);
      entry.timer = null;
      entry.remaining = Math.max(0, entry.deadline - performance.now());
      entry.deadline = null;
    }
  }, []);

  /** 指针移开后按**剩余时长**续时（不是重新给满一个 TTL，那会让人误以为提示不再消散）。 */
  const resumeDismiss = useCallback(
    (ids: string[]) => {
      for (const id of ids) {
        const entry = timersRef.current.get(id);
        if (!entry || entry.timer !== null || entry.remaining === null) {
          continue;
        }
        arm(id, entry.remaining);
      }
    },
    [arm],
  );

  return { items, dismiss, pauseDismiss, resumeDismiss, unreadCount, refreshUnreadCount };
}
