/**
 * 本地工作区改进记录（PRD-09-076 / PRD-07-070 / TDD-12 §2.1）。
 *
 * 与知识度量那一块**形态相同、数据与开关完全独立**：两个开关互不启停、两份数据各自清除。
 * 放在同一节里，是因为用户问的是同一个问题——"这台机器上记了我什么"。
 *
 * **默认关闭，且关着时这里如实说"还没有记录"**：一个默认关闭的功能，面板上却显示着
 * 一堆零，会让人以为它在记只是没数据。
 */

import { useCallback, useEffect, useState } from "react";
import { SelectBox } from "../../components/ui/SelectBox";
import { useTranslation } from "react-i18next";
import {
  commands,
  events,
  type LocalWorkspaceMetricsDto,
  type WorkspaceMetricAggregateDto,
  type WorkspaceMetricRatioDto,
  type WorkspaceMetricsAvailabilityDto,
  type WorkspaceMetricsExportFormatDto,
} from "../../ipc/bindings";
import { useConfirm } from "../../components/ConfirmHost";
import { vaultErrorText } from "../../lib/vault-error";
import { goTo } from "../../lib/navigate";

/** 比率的呈现：样本不足时**只给分子分母**，绝不显示一个骗人的百分数。 */
function ratioText(ratio: WorkspaceMetricRatioDto, language: string, fallback: string): string {
  const { numerator, denominator, value } = ratio;
  if (value === null) {
    return `${numerator}/${denominator} · ${fallback}`;
  }
  return `${new Intl.NumberFormat(language, { style: "percent", maximumFractionDigits: 0 }).format(
    value,
  )} (${numerator}/${denominator})`;
}

/**
 * 读不到观测时的**如实说明 + 可行动出口**（TDD-12 §2.1「schema 代际、可用性出口与回退语义」）。
 *
 * **这两态各有各的出口，不能合成一句"暂时不可用"**：
 * - `storage_unavailable` 是"此刻拿不到"——重试即可，且不对原因作超出已知事实的承诺；
 * - `schema_invalid` 是"我们不认识这个库，也不会去动它"（fail closed）。它**必须把用户
 *   指向显式清除**——清除重建是唯一恢复路径，不给出口就是让他对着一个永远空的面板无从
 *   下手，而"我们没删你的东西"这句话反而变成了牢门。
 *
 * 拆成独立组件是为了**可测**：面板本体要一次 IPC 往返才有数据，静态渲染下永远是 null。
 */
export function WorkspaceMetricsUnavailableNotice({
  availability,
  onRetry,
  onClear,
}: {
  availability: WorkspaceMetricsAvailabilityDto;
  onRetry: () => void;
  onClear: () => void;
}) {
  const { t } = useTranslation("settings");
  if (availability.kind !== "unavailable") {
    return null;
  }
  const schemaInvalid = availability.code === "schema_invalid";
  return (
    <div className="luna-settings-panel__desc" role="status">
      <p>
        {schemaInvalid
          ? t("workspace_metrics.unavailable.schemaInvalid")
          : t("workspace_metrics.unavailable.storageUnavailable")}
      </p>
      <button
        type="button"
        className="luna-btn luna-btn--secondary luna-btn--sm"
        onClick={schemaInvalid ? onClear : onRetry}
      >
        {schemaInvalid
          ? t("workspace_metrics.unavailable.clearAndRebuild")
          : t("workspace_metrics.unavailable.retry")}
      </button>
    </div>
  );
}

export function LocalWorkspaceMetricsPanel() {
  const ask = useConfirm();
  const { t, i18n } = useTranslation("settings");
  const [data, setData] = useState<LocalWorkspaceMetricsDto | null>(null);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [exportFormat, setExportFormat] = useState<WorkspaceMetricsExportFormatDto>("json");

  const refresh = useCallback(() => {
    void commands.localWorkspaceMetricsSummary(null).then((result) => {
      setData(result.status === "ok" ? result.data : null);
    });
  }, []);

  // 清除走 `refresh()`：清除命令返回后 Rust 会广播一次 availability，但面板上的**行**
  // 也变了，而事件按契约只带 enabled/availability——故这里仍要重查一次摘要。
  const clearNow = useCallback(() => {
    // 二次确认（PRD-09-076）：清除不可撤销，且它删的是用户自己的记录。
    void ask({
      title: t("workspace_metrics.clear_title"),
      description: t("workspace_metrics.clear_desc"),
      confirmLabel: t("workspace_metrics.clear"),
      cancelLabel: t("workspace_metrics.cancel"),
    }).then((confirmed: boolean) => {
      if (!confirmed) return;
      void commands.clearLocalWorkspaceMetrics().then((result) => {
        setFeedback(result.status === "ok" ? t("workspace_metrics.cleared") : result.error);
        refresh();
      });
    });
  }, [ask, refresh, t]);

  useEffect(() => {
    // 挂载即查一次（§2.1 末条：面板挂载、重新聚焦、显式刷新、导出/清除后重查摘要）。
    refresh();
    // 开关一变就重取：面板上的"还没有记录"与开关状态必须同时更新，否则用户刚打开开关
    // 却看到一句"未开启"。
    const unlistenSettings = events.settingsChanged.listen((event) => {
      if (event.payload.key === "privacy.local_workspace_metrics") refresh();
    });
    // **存储状态变了要能被面板看见**：坏 schema、清除完成这些事实此前只存在于 DTO 里，
    // 而面板从不重查——用户按完「清除」看到的还是上一帧。
    const unlistenAvailability = events.workspaceMetricsAvailability.listen(() => refresh());
    // 重新聚焦也重查：面板可能在后台开了很久，而观测库在这期间被另一条路径清掉了。
    window.addEventListener("focus", refresh);
    return () => {
      void unlistenSettings.then((dispose) => dispose());
      void unlistenAvailability.then((dispose) => dispose());
      window.removeEventListener("focus", refresh);
    };
  }, [refresh]);

  if (!data) {
    return null;
  }

  const rows = data.rows;
  const unavailable = data.availability.kind === "unavailable";
  const tooFewSamples = t("workspace_metrics.tooFewSamples", {
    count: data.minimum_ratio_samples,
  });
  return (
    <section
      id="settings-section-workspace_metrics"
      tabIndex={-1}
      className="luna-settings-panel"
      aria-label={t("workspace_metrics.title")}
    >
      <header className="luna-settings-panel__header">
        <h2>{t("workspace_metrics.title")}</h2>
        <p className="luna-settings-panel__meta">
          {t("workspace_metrics.window", {
            start: data.range.start_day,
            end: data.range.end_day,
            retention: data.retention_days,
          })}
        </p>
      </header>

      {/* **读不到时先把话说清楚并给出口**，而不是让用户对着一句"还没有记录"发呆：
          那句话在坏 schema 下是**假的**——库里有东西，只是我们不认识它。 */}
      <WorkspaceMetricsUnavailableNotice
        availability={data.availability}
        onRetry={refresh}
        onClear={clearNow}
      />

      {unavailable ? null : rows.length === 0 ? (
        // 关着与"开着但还没数据"是两句话：前者要说"它没在记"，后者要说"还没有可看的"。
        <p className="luna-settings-panel__desc">
          {data.enabled ? t("workspace_metrics.empty") : t("workspace_metrics.disabled")}
          {!data.enabled ? <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm"
            onClick={() => goTo("settings://privacy/privacy.local_workspace_metrics")}>
            {t("workspace_metrics.show_setting")}
          </button> : null}
        </p>
      ) : (
        <ul className="luna-settings-panel__rows">
          {rows.map((row) => (
            <li key={rowKey(row)} className="luna-settings-panel__row">
              {row.kind === "surface_viewed" && (
                <>
                  <span>{t(`workspace_metrics.surface.${row.surface}`)}</span>
                  <span>{t("workspace_metrics.visits", { count: row.count })}</span>
                </>
              )}
              {row.kind === "create_entry" && (
                <>
                  <span>
                    {t(`workspace_metrics.action.${row.action}`)} ·{" "}
                    {t(`workspace_metrics.entry.${row.entry}`)}
                  </span>
                  <span>{ratioText(row.ratio, i18n.language, tooFewSamples)}</span>
                </>
              )}
              {row.kind === "quick_note_settled" && (
                <>
                  <span>{t("workspace_metrics.settled")}</span>
                  <span>{t("workspace_metrics.times", { count: row.count })}</span>
                </>
              )}
              {row.kind === "material_import" && (
                <>
                  <span>{t("workspace_metrics.materialImport")}</span>
                  <span>{ratioText(row.ratio, i18n.language, tooFewSamples)}</span>
                </>
              )}
              {row.kind === "status_visit" && (
                <>
                  <span>{t(`workspace_metrics.status.${row.surface}`)}</span>
                  {/* **两个数一起给**：到访次数是这条路径"有没有人来"，短停留未操作占比
                      是"来了有没有用起来"。只给后者会让一个 1/2 看起来像个结论。 */}
                  <span>
                    {t("workspace_metrics.visits", { count: row.total })} ·{" "}
                    {t("workspace_metrics.shortWithoutAction")}{" "}
                    {ratioText(row.ratio, i18n.language, tooFewSamples)}
                  </span>
                </>
              )}
            </li>
          ))}
        </ul>
      )}

      <div className="luna-settings-panel__actions">
        {/* 两种格式各有各的用处：JSON 结构保真（判别联合不被压平），CSV 表格工具直接
            打得开。选择器而不是两颗按钮——它们是同一个动作的两种形态。 */}
        <SelectBox
          className="luna-settings-field__select"
          value={exportFormat}
          ariaLabel={t("workspace_metrics.format")}
          onChange={(event) =>
            setExportFormat(event.target.value as WorkspaceMetricsExportFormatDto)
          }
        >
          {/* 格式名是**产品事实**（文件扩展名），不是可翻译文案——但它仍必须经 i18n
              通道，否则下一个人会以为"这里可以直接写字"。 */}
          <option value="json">{t("workspace_metrics.formatJson")}</option>
          <option value="csv">{t("workspace_metrics.formatCsv")}</option>
        </SelectBox>
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          disabled={rows.length === 0}
          onClick={() => {
            void commands.exportLocalWorkspaceMetrics(null, exportFormat).then((result) => {
              if (result.status !== "ok") {
                setFeedback(vaultErrorText(result.error));
                return;
              }
              // 三种结局各说各的：`exported` 报行数，`empty` 说没有可导出的内容，
              // `cancelled` 什么都不说——用户自己按的取消，再提示一次是噪音。
              if (result.data.kind === "exported") {
                setFeedback(t("workspace_metrics.exported", { count: result.data.row_count }));
              } else if (result.data.kind === "empty") {
                setFeedback(t("workspace_metrics.empty"));
              }
            });
          }}
        >
          {t("workspace_metrics.export")}
        </button>
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          // **读不到时清除必须仍然可按**：坏 schema 下没有任何一行，而清除重建正是
          // §2.1 给出的唯一恢复路径。按行数灰掉它，等于亲手把那扇门锁上——用户看到的是
          // 一个永远空的面板，和一颗永远灰着的按钮。
          disabled={rows.length === 0 && !unavailable}
          onClick={clearNow}
        >
          {t("workspace_metrics.clear")}
        </button>
      </div>
      {feedback && (
        <p className="luna-settings-panel__desc" role="status">
          {feedback}
        </p>
      )}
    </section>
  );
}

function rowKey(row: WorkspaceMetricAggregateDto): string {
  switch (row.kind) {
    case "surface_viewed":
      return `surface:${row.surface}`;
    case "create_entry":
      return `entry:${row.action}:${row.entry}`;
    case "quick_note_settled":
      return "settled";
    case "material_import":
      return "import";
    case "status_visit":
      return `status:${row.surface}`;
  }
}
