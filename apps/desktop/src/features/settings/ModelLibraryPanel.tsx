/**
 * 模型库：这一页上的一张卡片，长度由页签 + 分页封顶（ADR-126 第 7 条推翻了 ADR-122 第 1
 * 条的浮层形态）。`settings://local_models/<尾段>` 落到这一页，由本面板切页签。
 */

import { useTranslation } from "react-i18next";

import { formatBytes } from "../../lib/format-bytes";
import { isInstalled, ModelManagementPanel } from "./ModelManagementPanel";
import { useModels } from "./use-settings";

/** 页上那一块：装了几个、占多少，以及三个页签的正文。 */
export function ModelLibraryPanel({
  advancedMode,
  focusKey,
}: {
  advancedMode: boolean;
  focusKey: string | null;
}) {
  const { t, i18n } = useTranslation("settings");
  const controller = useModels();
  const { models, storage } = controller;
  // 判据与清单共用 `isInstalled`，不在这里再写一遍「什么叫已装」。
  const installed = models.filter(isInstalled).length;
  return (
    <section className="luna-settings-panel luna-settings-card" aria-label={t("models.title")}>
      <header className="luna-settings-panel__header">
        <h2>{t("models.title")}</h2>
        {storage ? (
          <span className="luna-settings-panel__meta">
            {t("models.library.summary", {
              count: installed,
              size: formatBytes(storage.total_on_disk_bytes, i18n.language),
            })}
          </span>
        ) : null}
      </header>
      <ModelManagementPanel
        advancedMode={advancedMode}
        focusKey={focusKey}
        controller={controller}
      />
    </section>
  );
}
