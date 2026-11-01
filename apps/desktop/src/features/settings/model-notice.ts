/**
 * 模型管理的「后端结果码 → 文案」：`CODE: detail` 形状的消息按 `models.notice.<CODE>` 取文案，
 * 没有登记的码原样露出（不吞信息）。模型清单与本地推理引擎一节共用这一份。
 */
export function noticeText(
  message: string,
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  const [code, ...rest] = message.split(": ");
  const detail = rest.join(": ");
  const key = `models.notice.${code}`;
  const translated = t(key, { detail, defaultValue: "" });
  return translated || message;
}
