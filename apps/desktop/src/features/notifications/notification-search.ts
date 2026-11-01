// 通知历史的**搜索基质**（纯函数，PRD-15-060）。
//
// **此前搜的是机器串**：`${class} ${severity} ${i18n_key} ${target}` ——也就是
// `completion` / `failure` / `recording_disk_low` / `settings://…`，而屏幕上写着的是
// 「磁盘空间不足」。中文界面下这个搜索框**一条都匹配不到**：用户照着占位符「按类型、
// 级别或目标搜索」输入「失败」「录音」「磁盘」，全部落空。一个搜不到任何可见文字的
// 搜索框，比没有搜索框更糟——它会让人以为"历史里没有这条"。
//
// 现在搜的是**用户读得到的那些字**：标题 + 正文 + 类别标签 + 级别标签。技术 key 与深链
// 目标仍留在末尾——它们对用户无意义，但对着日志排查的人正需要按 kind 搜到那一条。

/** 一条通知在当前界面语言下的四段可读文字（由 `useNotificationLabels` 取好）。 */
export interface NotificationLabels {
  title: string;
  body: string;
  classLabel: string;
  severityLabel: string;
}

/** 一条通知的技术标识（不随界面语言变）。 */
export interface NotificationIdentity {
  i18n_key: string;
  target: string | null;
}

/**
 * 拼出一条通知的可搜索文本。
 *
 * 顺序即权重意图：可读文字在前（模糊匹配按位置给分，见 `fuzzy.ts`），技术串垫底。
 */
export function notificationSearchText(
  identity: NotificationIdentity,
  labels: NotificationLabels,
): string {
  return [
    labels.title,
    labels.body,
    labels.classLabel,
    labels.severityLabel,
    identity.i18n_key,
    identity.target ?? "",
  ]
    .filter((part) => part !== "")
    .join(" ");
}
