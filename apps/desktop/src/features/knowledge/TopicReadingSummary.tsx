/**
 * 主题阅读页头的汇总行（ADR-134 / TDD-07 §4.1e「范围 → 摘要 → 声明与出处」）。
 *
 * **前端零业务**（ADR-003）：范围、声明与分歧数、哪些来源需要核对，全部由 Rust
 * `topic_reading_summary` 从主题 Markdown 与此刻的来源文件派生；这里只渲染。
 * 旧格式/坏证据没有汇总（`null`），不渲染——不能把「无法解析」画成「零条声明」。
 * 分歧与待核对只在有内容时出现，短页不留空壳；两者的计数兼作「只看这些」的筛选开关，
 * 筛选本身（哪条算分歧、哪条需核对）同样由 Rust 判定，宿主据此换成声明清单。
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  type TopicClaimFilterDto,
  type TopicReadingSummaryDto,
} from "../../ipc/bindings";
import { useVaultReload } from "../../hooks/use-vault-reload";

export interface TopicReadingSummaryProps {
  path: string;
  onOpenPath: (path: string) => void;
  onOpenTag: (tag: string) => void;
  /** 当前筛选；`null` = 显示全文。 */
  filter: TopicClaimFilterDto | null;
  onFilterChange: (filter: TopicClaimFilterDto | null) => void;
}

export function TopicReadingSummary({
  path,
  onOpenPath,
  onOpenTag,
  filter,
  onFilterChange,
}: TopicReadingSummaryProps) {
  const { t } = useTranslation("workspace");
  const [summary, setSummary] = useState<TopicReadingSummaryDto | null>(null);
  /** 迟到响应不串页：只认最近一次请求。 */
  const requestRef = useRef(0);

  const load = () => {
    const request = ++requestRef.current;
    void commands.topicReadingSummary(path).then((result) => {
      if (request === requestRef.current) {
        setSummary(result.status === "ok" ? result.data : null);
      }
    });
  };

  useEffect(() => {
    setSummary(null);
    load();
    // `load` 每次渲染都是新的一份；只在切页时重取，文件变化由下方的库刷新负责。
  }, [path]);

  useVaultReload(load);

  if (summary === null) {
    return null;
  }
  const { scope } = summary;
  return (
    <section className="luna-topic-summary" aria-label={t("topicSummary.label")}>
      <p className="luna-topic-summary__scope">
        <span className="luna-topic-summary__label">{t("topicSummary.scope")}</span>
        <span className="luna-topic-summary__title">{scope.title}</span>
        {scope.includes.map((tag) => (
          <button
            key={tag}
            type="button"
            className="luna-topic-summary__tag"
            onClick={() => onOpenTag(tag)}
          >
            #{tag}
          </button>
        ))}
        {scope.excludes.length > 0 && (
          <span className="luna-topic-summary__muted">
            {t("topicSummary.excludes", { tags: scope.excludes.map((tag) => `#${tag}`).join(" ") })}
          </span>
        )}
      </p>
      {scope.description !== "" && (
        <p className="luna-topic-summary__description">{scope.description}</p>
      )}
      <p className="luna-topic-summary__counts">
        <span>{t("topicSummary.claims", { count: summary.claims })}</span>
        {summary.conflicts > 0 && (
          <FilterToggle
            active={filter === "conflicts"}
            onToggle={() => onFilterChange(filter === "conflicts" ? null : "conflicts")}
          >
            {t("topicSummary.conflicts", { count: summary.conflicts })}
          </FilterToggle>
        )}
        {summary.transcript_records > 0 && (
          <FilterToggle
            active={filter === "transcript_records"}
            onToggle={() =>
              onFilterChange(filter === "transcript_records" ? null : "transcript_records")
            }
          >
            {t("topicSummary.transcriptRecords", { count: summary.transcript_records })}
          </FilterToggle>
        )}
        {summary.attention_claims > 0 && (
          <FilterToggle
            active={filter === "needs_check"}
            onToggle={() => onFilterChange(filter === "needs_check" ? null : "needs_check")}
          >
            {t("topicSummary.needsCheck", { count: summary.attention_claims })}
          </FilterToggle>
        )}
        {/* 升级前的旧内容或人工补写在声明块之外，没有逐条出处：如实标「未核验」，不猜出处。 */}
        {summary.unverified_lines > 0 && (
          <span className="luna-topic-summary__muted">
            {t("topicSummary.unverified", { count: summary.unverified_lines })}
          </span>
        )}
      </p>
      {summary.attention.length > 0 && (
        <div className="luna-topic-summary__attention">
          <p className="luna-topic-summary__attention-title">{t("topicSummary.attentionTitle")}</p>
          <ul className="luna-topic-summary__attention-list">
            {summary.attention.map((item) => (
              <li
                key={`${item.state}:${item.source_path ?? ""}`}
                className="luna-topic-summary__attention-item"
                data-state={item.state}
              >
                {item.source_path !== null && item.state !== "missing" ? (
                  <button
                    type="button"
                    className="luna-topic-summary__source"
                    onClick={() => onOpenPath(item.source_path as string)}
                  >
                    {item.source_title}
                  </button>
                ) : (
                  <span className="luna-topic-summary__source-text">
                    {item.source_title ?? t("topicSummary.unresolvedSource")}
                  </span>
                )}
                <span>{t(`topicSummary.state.${item.state}`, { count: item.claims })}</span>
              </li>
            ))}
          </ul>
          {summary.attention_omitted > 0 && (
            <p className="luna-topic-summary__muted">
              {t("topicSummary.attentionOmitted", { count: summary.attention_omitted })}
            </p>
          )}
        </div>
      )}
    </section>
  );
}

/** 计数兼筛选开关：按下即「只看这些」，再按一次回到全文。 */
function FilterToggle({
  active,
  onToggle,
  children,
}: {
  active: boolean;
  onToggle: () => void;
  children: string;
}) {
  return (
    <button
      type="button"
      className="luna-topic-summary__filter"
      aria-pressed={active}
      onClick={onToggle}
    >
      {children}
    </button>
  );
}
