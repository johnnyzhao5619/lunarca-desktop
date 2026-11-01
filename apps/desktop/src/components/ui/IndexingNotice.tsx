/**
 * 「索引中（x / y），结果可能不全」——开库那一轮增量扫描期间，读索引的视图顶上的同一句提示
 * （PRD-06-004 ④，TDD-06 §4.1；知识库与体检在 V1b 接入）。没在扫描、或这一轮什么都没变时不渲染。
 *
 * 扫描结束后视图怎么重取由 `useVaultReload` 负责；这里只说「此刻可能不全」。
 */

import { useTranslation } from "react-i18next";
import { useIndexStatus } from "../../hooks/use-index-status";

export function IndexingNotice() {
  const { t } = useTranslation("common");
  const status = useIndexStatus();
  if (!status) return null;
  return (
    <p className="luna-indexing-notice" role="status">
      {t("status.indexingPartial", { indexed: status.indexed, total: status.total })}
    </p>
  );
}
