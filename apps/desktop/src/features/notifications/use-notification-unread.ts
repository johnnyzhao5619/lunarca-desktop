import { useEffect, useState } from "react";
import { commands, events } from "../../ipc/bindings";
import { NOTIFICATIONS_CHANGED_EVENT } from "../../lib/app-events";

/**
 * 未读通知数（状态栏铃铛角标的取数口径——通知未读只在状态栏表达，PRD-07-002b）。
 * 后端派发通知事件（notification://event）与前端标已读/清空（NOTIFICATIONS_CHANGED_EVENT，
 * 后端标已读命令不发事件）两条来源都会触发重取。
 */
export function useNotificationUnread(): number {
  const [count, setCount] = useState(0);

  useEffect(() => {
    let disposed = false;
    const refresh = () => {
      void commands.notificationUnreadCount().then((next) => {
        if (!disposed) {
          setCount(next);
        }
      });
    };
    refresh();
    const unlisten = events.notificationEvent.listen(refresh);
    window.addEventListener(NOTIFICATIONS_CHANGED_EVENT, refresh);
    return () => {
      disposed = true;
      void unlisten.then((dispose) => {
        dispose();
      });
      window.removeEventListener(NOTIFICATIONS_CHANGED_EVENT, refresh);
    };
  }, []);

  return count;
}
