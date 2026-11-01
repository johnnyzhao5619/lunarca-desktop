/**
 * 分页控件：上一页 / 第 x 共 y 页 / 下一页。
 *
 * 只有一页时**什么都不渲染**——一排点不动的按钮加一句「第 1 / 1 页」是噪声
 * （判据同 `ListLoadMore` 的「已到底就不渲染」，以及 ADR-125 第 12 条）。
 *
 * 与 `ListLoadMore` 的分工：那一颗是**游标式取数**的列尾动作（还要再问后端要一页），
 * 这一枚切的是**已经在手的数组**（见 `lib/paginate.ts` 的头注）。
 */

import { useTranslation } from "react-i18next";

export function Pager({
  page,
  pageCount,
  onPageChange,
  label,
}: {
  /** 当前页（0 基）。 */
  page: number;
  pageCount: number;
  onPageChange: (page: number) => void;
  /** 这一组按钮管的是哪份列表（读屏用；同屏可能有两份分页）。 */
  label: string;
}) {
  const { t } = useTranslation("common");
  if (pageCount <= 1) return null;
  return (
    <nav className="luna-pager" aria-label={label}>
      <button
        type="button"
        className="luna-btn luna-btn--secondary luna-btn--sm"
        disabled={page <= 0}
        onClick={() => onPageChange(page - 1)}
      >
        {t("pager.previous")}
      </button>
      {/* 页码是状态不是控件：`role="status"` 让读屏在翻页后念出新位置，而不必去数按钮。 */}
      <span className="luna-pager__position" role="status">
        {t("pager.position", { page: page + 1, count: pageCount })}
      </span>
      <button
        type="button"
        className="luna-btn luna-btn--secondary luna-btn--sm"
        disabled={page >= pageCount - 1}
        onClick={() => onPageChange(page + 1)}
      >
        {t("pager.next")}
      </button>
    </nav>
  );
}
