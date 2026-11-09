/**
 * 本机知识度量面板（PRD-16 / Phase 11，「数据 / 隐私」分组专属面板）。
 *
 * 只渲染 Rust 汇总 DTO；不读取事件、不接触 token，也不从百分比反推分子分母。
 *
 * 呈现上它与同组的设置行**同一套语言**（`.luna-settings-panel` 原语）：五项指标是
 * 「左边一句人话 + 右边一个事实」的信息行，与设置行的栅格同源；导出是面板内的一节，
 * 不是一个原生 `fieldset`（那圈带缺口的浏览器默认边框在这一页里没有第二处）；清除是
 * 一条危险动作行，说明在左、按钮在右——与其余每一行"说明在左、控件在右"读法一致。
 */

import { useCallback, useEffect, useState, type ReactNode } from "react";
import { SelectBox } from "../../components/ui/SelectBox";
import { useTranslation } from "react-i18next";
import { useConfirm } from "../../components/ConfirmHost";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { ChevronRightIcon } from "@lunarca/editor-core/Icon";
import {
  commands,
  events,
  type KnowledgeMetricsExportFormatDto,
  type LocalKnowledgeMetricsDto,
  type MetricRatioDto,
} from "../../ipc/bindings";
import { HelpDetails } from "../../components/ui/HelpDetails";

function percent(value: number | null, locale: string): string | null {
  if (value === null) return null;
  return new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: 1 }).format(
    value / 10_000,
  );
}

/** 一条信息行：左侧标题（可带一行注脚），右侧是 Rust 算好的事实。 */
function MetricRow({
  label,
  value,
  details,
}: {
  label: string;
  value: string;
  /** 注脚（截尾 / 观察中 / 来源分布…）：空串一律不渲染，不留空行。 */
  details?: readonly string[];
}) {
  const shown = (details ?? []).filter((line) => line.length > 0);
  return (
    <div className="luna-settings-panel__row">
      <div className="luna-settings-panel__row-label">
        <span className="luna-settings-panel__row-title">{label}</span>
        {shown.map((line) => (
          <p key={line} className="luna-settings-panel__row-desc">
            {line}
          </p>
        ))}
      </div>
      <span className="luna-settings-panel__row-value">{value}</span>
    </div>
  );
}

/** 五项指标全都没有可算的样本（Rust 下发空比例 / 零次尝试）。 */
function noSamples(data: LocalKnowledgeMetricsDto): boolean {
  return (
    data.suggestion_percentage_basis_points === null &&
    data.topic_median_minutes === null &&
    data.linked_within_24h.percentage_basis_points === null &&
    data.citation_attempts === 0 &&
    data.model_supplement.percentage_basis_points === null
  );
}

/** 没有样本时把逐项指标收进按需展开；有样本时原样平铺。 */
function MetricsBody({
  collapsed,
  summary,
  children,
}: {
  collapsed: boolean;
  summary: string;
  children: ReactNode;
}) {
  return collapsed ? <details className="luna-settings-panel__note"><summary>{summary}</summary>{children}</details> : <>{children}</>;
}

export function LocalKnowledgeMetricsPanel() {
  const ask = useConfirm();
  const { t, i18n } = useTranslation("settings");
  const [data, setData] = useState<LocalKnowledgeMetricsDto | null>(null);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [startDay, setStartDay] = useState("");
  const [endDay, setEndDay] = useState("");
  const [exportFormat, setExportFormat] = useState<KnowledgeMetricsExportFormatDto>("json");

  const refresh = useCallback(
    (resetRange = false) =>
      void commands.localKnowledgeMetricsSummary().then((next) => {
        setData(next);
        setStartDay((current) =>
          resetRange ? next.range_start_day || "" : current || next.range_start_day || "",
        );
        setEndDay((current) =>
          resetRange ? next.range_end_day || "" : current || next.range_end_day || "",
        );
      }),
    [],
  );
  useEffect(() => {
    refresh(true);
    const onFocus = () => refresh(false);
    window.addEventListener("focus", onFocus);
    const unlistenSwitched = events.vaultSwitched.listen(() => refresh(true));
    const unlistenAvailability = events.knowledgeMetricsAvailability.listen(() => refresh(false));
    const unlistenSettings = events.settingsChanged.listen((event) => {
      if (event.payload.key === "privacy.local_knowledge_metrics") refresh(false);
    });
    return () => {
      window.removeEventListener("focus", onFocus);
      void unlistenSwitched.then((dispose) => dispose());
      void unlistenAvailability.then((dispose) => dispose());
      void unlistenSettings.then((dispose) => dispose());
    };
  }, [refresh]);
  // 库变了就重取一次（合并同一阵变更，唯一实现见 `use-vault-reload`）。
  useVaultReload(() => refresh(false));

  const ratioValue = (ratio: MetricRatioDto) => {
    const rate = percent(ratio.percentage_basis_points, i18n.language);
    return rate === null
      ? t("knowledge_metrics.insufficient")
      : t("knowledge_metrics.ratio_value", {
          rate,
          numerator: ratio.numerator,
          denominator: ratio.denominator,
        });
  };

  const ready = data?.status === "ready";
  return (
    <section
      id="settings-section-knowledge_metrics"
      tabIndex={-1}
      className="luna-settings-panel"
      aria-label={t("knowledge_metrics.title")}
    >
      <header className="luna-settings-panel__header">
        <h2>{t("knowledge_metrics.title")}<HelpDetails summary={t("knowledge_metrics.title")}>
          <p>{t("knowledge_metrics.description")}</p>
        </HelpDetails></h2>
        {/* 汇总区间是这一整块数字的前提，故与标题同排——它不是某一项指标的注脚。 */}
        {data && ready && data.range_start_day !== null && data.range_end_day !== null ? (
          <span className="luna-settings-panel__meta">
            {t("knowledge_metrics.range", {
              start: data.range_start_day,
              end: data.range_end_day,
            })}
          </span>
        ) : null}
      </header>

      {!data ? <p className="luna-empty">{t("knowledge_metrics.loading")}</p> : null}
      {data && !ready ? (
        <div className="luna-settings-panel__section" role="status">
          <p className="luna-settings-panel__hint">
            {t(`knowledge_metrics.status.${data.status}`)}
          </p>
          {data.status !== "cleared" ? (
            <div className="luna-settings-panel__actions">
              <button
                type="button"
                className="luna-btn luna-btn--secondary"
                onClick={() => {
                  setFeedback(null);
                  void commands.retryLocalKnowledgeMetrics().then(() => refresh(false));
                }}
              >
                {t("knowledge_metrics.retry")}
              </button>
            </div>
          ) : null}
        </div>
      ) : null}

      {/* 一项样本都没有时只说一次（F21）：五行「数据还不够」不是信息，是噪声；各项指标仍可展开。
          「没有样本」由 Rust 的空值 / 零分母表达，这里不把未知当成 0。 */}
      {data && ready && noSamples(data) ? (
        <p className="luna-settings-panel__hint" role="status">
          {t("knowledge_metrics.no_samples")}
        </p>
      ) : null}
      {data && ready ? (
        <MetricsBody collapsed={noSamples(data)} summary={t("knowledge_metrics.show_metrics")}>
          <div className="luna-settings-panel__rows">
            <MetricRow
              label={t("knowledge_metrics.suggestions")}
              value={
                data.suggestion_percentage_basis_points === null
                  ? t("knowledge_metrics.insufficient")
                  : t("knowledge_metrics.ratio_value", {
                      rate: percent(data.suggestion_percentage_basis_points, i18n.language),
                      numerator: data.suggestion_accepted,
                      denominator: data.suggestion_shown,
                    })
              }
              details={[
                t("knowledge_metrics.suggestion_detail", {
                  dismissed: data.suggestion_dismissed,
                  rolledBack: data.suggestion_rolled_back,
                }),
              ]}
            />
            <MetricRow
              label={t("knowledge_metrics.topic_time")}
              value={
                data.topic_median_minutes === null
                  ? t("knowledge_metrics.insufficient")
                  : t("knowledge_metrics.topic_value", {
                      minutes: data.topic_median_minutes,
                      completed: data.topic_completed,
                    })
              }
              details={[
                t("knowledge_metrics.sample_detail", {
                  censored: data.topic_censored,
                  open: data.topic_open,
                }),
              ]}
            />
            <MetricRow
              label={t("knowledge_metrics.linked")}
              value={ratioValue(data.linked_within_24h)}
              details={[
                t("knowledge_metrics.window_detail", {
                  censored: data.linked_within_24h.censored,
                  open: data.linked_within_24h.open,
                }),
                t("knowledge_metrics.link_origin_detail", {
                  automatic: data.linked_automatic,
                  confirmed: data.linked_confirmed,
                  manual: data.linked_manual,
                }),
              ]}
            />
            <MetricRow
              label={t("knowledge_metrics.citations")}
              value={
                data.citation_attempts === 0
                  ? t("knowledge_metrics.insufficient")
                  : t("knowledge_metrics.citation_value", {
                      succeeded: data.citation_succeeded,
                      attempts: data.citation_attempts,
                    })
              }
              details={[
                t("knowledge_metrics.citation_detail", {
                  stale: data.citation_stale_anchor,
                  missing: data.citation_source_missing,
                }),
              ]}
            />
            <MetricRow
              label={t("knowledge_metrics.supplement")}
              value={ratioValue(data.model_supplement)}
            />
          </div>

          <details className="luna-settings-metrics__trend">
            <summary>
              <ChevronRightIcon size={12} />
              {t("knowledge_metrics.trend")}
            </summary>
            {data.trend.length === 0 ? (
              <p className="luna-settings-panel__hint">{t("knowledge_metrics.insufficient")}</p>
            ) : (
              <table>
                <thead>
                  <tr>
                    <th>{t("knowledge_metrics.trend_day")}</th>
                    <th>{t("knowledge_metrics.trend_metric")}</th>
                    <th>{t("knowledge_metrics.trend_value")}</th>
                  </tr>
                </thead>
                <tbody>
                  {data.trend.map((row) => (
                    <tr key={`${row.day}:${row.metric}`}>
                      <td>{row.day}</td>
                      <td>{t(`knowledge_metrics.metric.${row.metric}`)}</td>
                      <td>
                        {row.denominator === 0
                          ? t("knowledge_metrics.trend_count", {
                              numerator: row.numerator,
                              censored: row.censored,
                            })
                          : t("knowledge_metrics.ratio_value", {
                              rate: percent(row.percentage_basis_points, i18n.language),
                              numerator: row.numerator,
                              denominator: row.denominator,
                            })}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </details>
        </MetricsBody>
      ) : null}

      {/* 导出：面板内的一节。`fieldset` 保留的是它的**语义**（未就绪时整组不可用），
          外观走面板节的语言——原生 legend 那圈带缺口的边框在这一页里没有第二处。 */}
      {/* `fieldset` 只借它「整组不可用」的语义，**不用 `<legend>`**：legend 会浮到
          边框上并在那里咬出一个缺口（真机可见），那是它与原生凹槽边框配套的行为，
          套进设计系统的卡片里就成了一处豁口。组名改由 `aria-labelledby` 指向标题本身。 */}
      <fieldset
        className="luna-settings-panel__section luna-settings-panel__block luna-settings-metrics__export"
        aria-labelledby="knowledge-metrics-export-title"
        disabled={!ready}
      >
        <span id="knowledge-metrics-export-title" className="luna-settings-panel__section-title">
          {t("knowledge_metrics.export_range")}
        </span>
        <div className="luna-settings-metrics__export-controls">
          <label>
            <span>{t("knowledge_metrics.start_day")}</span>
            <input
              className="luna-settings-field__input"
              type="date"
              value={startDay}
              onChange={(event) => setStartDay(event.target.value)}
            />
          </label>
          <label>
            <span>{t("knowledge_metrics.end_day")}</span>
            <input
              className="luna-settings-field__input"
              type="date"
              value={endDay}
              onChange={(event) => setEndDay(event.target.value)}
            />
          </label>
          <label>
            <span>{t("knowledge_metrics.export_format")}</span>
            <SelectBox
              className="luna-settings-field__select"
              value={exportFormat}
              onChange={(event) =>
                setExportFormat(event.target.value as KnowledgeMetricsExportFormatDto)
              }
            >
              <option value="json">{t("knowledge_metrics.export_format_json")}</option>
              <option value="csv">{t("knowledge_metrics.export_format_csv")}</option>
            </SelectBox>
          </label>
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            onClick={() => {
              setFeedback(null);
              void commands
                .exportLocalKnowledgeMetrics(startDay || null, endDay || null, exportFormat)
                .then((result) => {
                  if (result.status === "error") setFeedback(t("knowledge_metrics.export_failed"));
                  else if (result.data === "exported") setFeedback(t("knowledge_metrics.exported"));
                });
            }}
          >
            {t("knowledge_metrics.export")}
          </button>
        </div>
      </fieldset>

      {/* 清除：与设置行同构的一行——说明在左，动作在右。此前是一颗裸红按钮加一行注脚，
          于是页面上唯一的破坏性动作反倒是唯一不按"说明 + 控件"读的那一块。 */}
      <div className="luna-settings-panel__rows">
        <div className="luna-settings-panel__row">
          <div className="luna-settings-panel__row-label">
            <span className="luna-settings-panel__row-title">
              {t("knowledge_metrics.clear_subject")}
            </span>
            <p className="luna-settings-panel__row-desc">{t("knowledge_metrics.clear_hint")}</p>
          </div>
          {/* 清除确认此前是本文件自绘的一张卡 + 一份手写的 Esc 与"圈住两颗按钮"的焦点
              环——全仓唯一有焦点圈定的地方，却也是唯一把可聚焦元素写死成两个的地方。
              改走共享确认卡后，Esc / 焦点圈定 / 焦点归还与其余破坏性动作同一份实现。 */}
          <button
            type="button"
            className="luna-btn luna-btn--danger"
            onClick={() => {
              void ask({
                title: t("knowledge_metrics.clear_confirm_title"),
                description: t("knowledge_metrics.clear_confirm"),
                confirmLabel: t("knowledge_metrics.clear_confirm_action"),
                cancelLabel: t("knowledge_metrics.cancel"),
              }).then((ok) => {
                if (!ok) {
                  return;
                }
                setFeedback(null);
                void commands.clearLocalKnowledgeMetrics().then((result) => {
                  if (result.status === "error") {
                    setFeedback(t("knowledge_metrics.clear_failed"));
                  } else setFeedback(t("knowledge_metrics.cleared"));
                  refresh();
                });
              });
            }}
          >
            {t("knowledge_metrics.clear")}
          </button>
        </div>
      </div>

      {feedback ? (
        <p className="luna-settings-panel__hint" role="status">
          {feedback}
        </p>
      ) : null}
    </section>
  );
}
