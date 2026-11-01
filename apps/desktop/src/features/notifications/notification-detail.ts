import { formatDateTime } from "../../lib/format-datetime";
// 通知详情的纯逻辑（NotificationDetail 的可无头测试内核）。
// 组件只负责把这里决定的字段映射为 i18n 标签 + 渲染（DRY，测试锁定分支）。

/** 详情元数据字段（渲染顺序即返回顺序）。 */
export type DetailMetaField = "channels" | "time";

/**
 * Unix 毫秒字符串 → 本地时间；无效（空/0/非数）返回空串。
 *
 * `locale` 必须由调用方注入（界面语言），此前直接 `toLocaleString()` 读的是**系统 locale**。
 */
export function formatNotificationTime(value: string, locale: string): string {
  return formatDateTime(value, locale);
}

/** 这条通知带没带有效时间戳（与 `formatDateTime` 同一判据，但不产出文本）。 */
export function hasTimestamp(value: string): boolean {
  const ms = Number.parseInt(value, 10);
  return Number.isFinite(ms) && ms > 0;
}

/**
 * 详情的**元数据表**该列哪几格：渠道仅在非空时列，时间仅在可解析时列。避免空行噪声
 * （如无渠道时不留「触达渠道：」空项）。
 *
 * **不含「类别」「级别」**：它们已经是卡片顶部那两枚徽标——同一事实在同一屏只呈现
 * 一次（PRD-07-065）。这张表此前六格里有两格是徽标的复读、一格恒定值，剩下的真信息被
 * 淹在里面。
 *
 * **不含「状态」**：打开详情即标已读（邮件式，PRD-15-061 明写），于是那一行在用户
 * 眼里**永远是「已读」**——一格恒定值不是信息，是噪声。已读态由列表上的未读标记表达，
 * 那里它才真的会变。
 */
export function visibleDetailFields(item: {
  channels: string[];
  created_at_ms: string;
}): DetailMetaField[] {
  const fields: DetailMetaField[] = [];
  if (item.channels.length > 0) {
    fields.push("channels");
  }
  // 这里问的是「有没有一个有效时间」，不是「把它格式化成什么样」——故不需要 locale。
  if (hasTimestamp(item.created_at_ms)) {
    fields.push("time");
  }
  return fields;
}
