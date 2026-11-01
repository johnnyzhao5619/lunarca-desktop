// 一条通知在当前界面语言下的四段可读文字——**列表行、右键菜单、搜索基质共用这一处**。
//
// 三处此前各算各的：行渲染算一遍 title/classLabel/severityLabel、右键菜单为了「复制
// 标题」再算一遍 title、而搜索**压根没算**（它拿机器串去匹配，见 `notification-search.ts`）。
// 同一条通知在同一屏上有三份"它叫什么"的算法，其中一份还是错的。

import { useCallback } from "react";
import { useTranslation } from "react-i18next";
import { resolveNotificationBaseKey } from "./notification-i18n";
import type { NotificationLabels } from "./notification-search";

/** 取标签所需的最小形状——`NotificationHistoryDto` 与 `NotificationEventDto` 都满足。 */
export interface NotificationLike {
  class: string;
  severity: string;
  i18n_key: string;
  params: Record<string, string>;
}

/**
 * 取一条通知的可读文字。类别/级别标签缺词条时回落到原始值（`defaultValue`），
 * 标题/正文缺词条时由 [resolveNotificationBaseKey] 回落到通用兜底——绝不把
 * `notifications:recording_retranscribed` 这种技术 key 摆给用户看。
 */
export function useNotificationLabels(): (item: NotificationLike) => NotificationLabels {
  const { t, i18n } = useTranslation("notifications");
  return useCallback(
    (item: NotificationLike) => {
      const baseKey = resolveNotificationBaseKey(item.i18n_key, i18n);
      return {
        title: t(`${baseKey}.title`, item.params),
        body: t(`${baseKey}.body`, item.params),
        classLabel: t(`history.class.${item.class}`, { defaultValue: item.class }),
        severityLabel: t(`history.severity.${item.severity}`, { defaultValue: item.severity }),
      };
    },
    [i18n, t],
  );
}
