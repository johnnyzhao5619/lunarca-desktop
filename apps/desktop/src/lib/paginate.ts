/**
 * 手上已有全部条目时的分页（纯函数）。
 *
 * **与 `features/workspace/paged-list.ts` 是两件事，别混用**：那一份是**游标式取数**
 * （Rust 一页一页给，「到底了」由服务端说），用于 Vault 里可能有几万条的笔记；这一份
 * 只切分**已经在手的数组**（用量记录一次取 200 条、模型目录十几条），不发任何请求。
 * 判据：要不要再问后端拿下一页。要，用那一份；不要，用这一份。
 *
 * 存在的理由是**页面高度**：设置页里几张卡片并列，其中一张铺开几百行会把其余几张挤到
 * 屏幕之外（ADR-126 第 7 条：内容回到卡片里，长度由分页封顶而不是靠浮层遮住）。
 */

/** 一页的结果。`page` 已被夹到合法范围——调用方不必自己钳。 */
export interface Page<T> {
  items: T[];
  /** 实际页码（0 基）。传入越界时它与传入值不同。 */
  page: number;
  /** 总页数；空列表恒为 1（"第 1 / 1 页"比"第 1 / 0 页"读得通）。 */
  pageCount: number;
}

/**
 * 切出第 `page` 页（0 基）。
 *
 * **页码越界一律夹住而不是报错**：条目会因为筛选、删除、刷新而变少，用户停在第 5 页时
 * 列表缩到只剩 2 页是常态。夹住的结果是"跳到最后一页"，而不是一片空白。
 */
export function paginate<T>(items: readonly T[], page: number, pageSize: number): Page<T> {
  const size = Math.max(1, Math.floor(pageSize));
  const pageCount = Math.max(1, Math.ceil(items.length / size));
  const safe = Math.min(Math.max(0, Math.floor(page)), pageCount - 1);
  return {
    items: items.slice(safe * size, safe * size + size),
    page: safe,
    pageCount,
  };
}
