/**
 * 服务端下发的标识符怎么变成界面标签（ADR-121 ③）。
 *
 * 登记过的用登记文案；没登记的**不把标识符放进标签位**——给中性标签，标识符降为副信息。
 *
 * 判据是「这句文案在不在」而不是另维护一张白名单：白名单会与文案表分叉，而分叉的那一刻
 * 用户看到的就是裸 key。
 */

/**
 * 权益 key 的 i18n 落点。点号换下划线：i18next 拿 `.` 当层级分隔符，而权益 key 天生带点
 * （`devices.max`），不换就得把文案表按服务端的点号嵌套成好几层。
 */
export function entitlementI18nKey(key: string): string {
  return `entitlement.${key.replace(/\./g, "_")}`;
}

export interface ServerLabel {
  label: string;
  /** 未登记时把原始标识符降到副信息位；登记过则没有这一行。 */
  hint?: string;
}

/**
 * @param lookup 取文案，未登记时必须返回空串（调用处传 `(k) => t(k, { defaultValue: "" })`）。
 * @param neutral 未登记时的中性标签。
 */
export function serverLabel(
  lookup: (i18nKey: string) => string,
  i18nKey: string,
  raw: string,
  neutral: string,
): ServerLabel {
  const registered = lookup(i18nKey);
  return registered ? { label: registered } : { label: neutral, hint: raw };
}

/**
 * 设备上限这条权益的落点是**设备段的标题**（`2 / 4`），不是权益清单里的一行（ADR-121 ②）。
 * 唯一一条有已知落点的权益键，且**失败软着陆**：服务端没下发它，标题就只是「设备」。
 */
export const DEVICE_LIMIT_ENTITLEMENT = "devices.max";

/**
 * 权益值里的非负整数；不是数字就当没有。值是服务端定义的任意 JSON（PRD-13 §1.2），
 * 故只认「能当上限用的形状」——把解析不出的值画成 `2 / NaN` 比不画更糟。
 */
export function entitlementCount(valueJson: string): number | null {
  try {
    const parsed: unknown = JSON.parse(valueJson);
    return typeof parsed === "number" && Number.isInteger(parsed) && parsed >= 0 ? parsed : null;
  } catch {
    return null;
  }
}

/**
 * 设备上限：`devices.max` 里那个能当上限用的整数，取不到就是没有上限。**唯一一份**——
 * 段标题与权益行的过滤判据必须同值，否则会「标题没用上它、权益行又把它藏了」两头落空。
 */
export function deviceLimitFrom(
  entitlements: { entitlements: { key: string; value_json: string }[] } | null,
): number | null {
  if (entitlements === null) return null;
  for (const item of entitlements.entitlements) {
    if (item.key !== DEVICE_LIMIT_ENTITLEMENT) continue;
    const count = entitlementCount(item.value_json);
    if (count !== null) return count;
  }
  return null;
}
