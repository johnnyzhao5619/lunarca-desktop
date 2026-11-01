/**
 * 分页列表的列尾控件（TDD-07 §4.1a）。
 *
 * 三种状态各说各的，一个都不许含糊：
 * - **还有下一页** → 「加载更多」（取数中禁用并如实说在取）。
 * - **取数失败** → 说明 + 「重试」+ 「重建索引」（PRD-07-023：失败可能是索引坏了，
 *   重试救不回来；重建走开库同一条后台扫描，终态自动刷新）。与「列表为空」是两件事：
 *   把失败画成空态，用户会以为库里没东西——而那正是本批要修掉的那类静默错误。
 * - **已到底** → 什么都不渲染。一条常驻的「没有更多了」是在给每份列表加一行噪音。
 *
 * **本期不引入虚拟列表**（契约里写死的显式取舍）：DOM 保护由「未加载的页不在 DOM」
 * 承担——页是用户主动加载的，滚到哪儿才有哪儿。虚拟列表要连带处理焦点、读屏与可变行高，
 * 规模超出这一档时再单独裁决。
 */

import { useTranslation } from "react-i18next";
import { commands } from "../../ipc/bindings";

export interface ListLoadMoreProps {
  loading: boolean;
  exhausted: boolean;
  failed: boolean;
  onLoadMore: () => void;
  onRetry: () => void;
}

/** 取数状态与条目来自同一分页宿主，空数组本身不能证明列表为空。 */
export type ListLoadState = Pick<ListLoadMoreProps, "loading" | "failed" | "exhausted">;

export function ListLoadMore({
  loading,
  exhausted,
  failed,
  onLoadMore,
  onRetry,
}: ListLoadMoreProps) {
  const { t } = useTranslation("workspace");

  if (failed) {
    return (
      <div className="luna-list-more">
        <span className="luna-list-more__notice" role="alert">
          {t("list.loadFailed")}
        </span>
        <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={onRetry}>
          {t("list.retry")}
        </button>
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          onClick={() => void commands.rebuildVaultIndex()}
        >
          {t("list.rebuildIndex")}
        </button>
      </div>
    );
  }

  if (exhausted) {
    return null;
  }

  return (
    <div className="luna-list-more">
      <button
        type="button"
        className="luna-btn luna-btn--secondary luna-btn--sm"
        disabled={loading}
        onClick={onLoadMore}
      >
        {loading ? t("list.loading") : t("list.loadMore")}
      </button>
    </div>
  );
}
