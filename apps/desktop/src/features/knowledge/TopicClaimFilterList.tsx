/**
 * 主题阅读视图的声明筛选清单（TDD-07 §4.1e「分歧/待更新可筛选」）。
 *
 * **前端零业务**：哪些声明算分歧、哪些需要核对、每条出处此刻是什么状态，全部来自 Rust
 * `topic_claims`；这里不解析 Markdown、不按类名或文本猜。点出处交回 Rust
 * `open_topic_citation` 按该出处的定位锚、版本优先打开；不是有效出处时退回按路径打开。
 * 引句按需展开（TDD-07 §4.1e「证据按需展开」）：默认只显示来源与状态，不把整页撑成引文墙。
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  type TopicClaimCitationDto,
  type TopicClaimFilterDto,
  type TopicClaimListDto,
} from "../../ipc/bindings";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { reportCitationOutcome } from "../../lib/app-events";
import { formatDate } from "../../lib/format-datetime";

const FILTER_HEADINGS = {
  conflicts: "topicSummary.filteredConflicts",
  needs_check: "topicSummary.filteredNeedsCheck",
  transcript_records: "topicSummary.filteredTranscripts",
} as const satisfies Record<TopicClaimFilterDto, string>;

export interface TopicClaimFilterListProps {
  path: string;
  filter: TopicClaimFilterDto;
  onClear: () => void;
  onOpenPath: (path: string) => void;
}

export function TopicClaimFilterList({
  path,
  filter,
  onClear,
  onOpenPath,
}: TopicClaimFilterListProps) {
  const { t } = useTranslation("workspace");
  /** `undefined` = 载入中；`null` = 此页没有可筛选的声明（旧格式等）。 */
  const [list, setList] = useState<TopicClaimListDto | null | undefined>(undefined);
  const requestRef = useRef(0);
  /** 展开了引句的声明。换页或换筛选即收起。 */
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());

  const load = () => {
    const request = ++requestRef.current;
    void commands.topicClaims(path, filter).then((result) => {
      if (request === requestRef.current) {
        setList(result.status === "ok" ? result.data : null);
      }
    });
  };

  useEffect(() => {
    setList(undefined);
    setExpanded(new Set());
    load();
    // 只在页面或筛选变化时重取；文件变化由库刷新负责。
  }, [path, filter]);
  useVaultReload(load);

  const openCitation = (citation: TopicClaimCitationDto) => {
    void commands.openTopicCitation(path, citation.citation_id).then((result) => {
      if (result.status === "ok" && result.data.status === "opened") {
        reportCitationOutcome({ title: result.data.title, outcome: result.data.outcome });
      } else if (citation.source_path !== null) {
        onOpenPath(citation.source_path);
      }
    });
  };

  const toggleQuotes = (claimId: string) => {
    setExpanded((current) => {
      const next = new Set(current);
      if (!next.delete(claimId)) next.add(claimId);
      return next;
    });
  };

  const heading =
    list === undefined || list === null
      ? null
      : t(FILTER_HEADINGS[filter], { count: list.claims.length + list.omitted });

  return (
    <section className="luna-topic-claims" aria-label={heading ?? undefined}>
      <header className="luna-topic-claims__header">
        {heading !== null && <p className="luna-topic-claims__title">{heading}</p>}
        <button type="button" className="luna-btn luna-btn--secondary" onClick={onClear}>
          {t("topicSummary.showAll")}
        </button>
      </header>
      {filter === "conflicts" && (
        <p className="luna-topic-summary__muted">{t("topicSummary.conflictsHint")}</p>
      )}
      {list === undefined ? (
        <p className="luna-empty" role="status">
          {t("common:status.loading")}
        </p>
      ) : list === null || list.claims.length === 0 ? (
        <p className="luna-empty" role="status">
          {t("topicSummary.filterEmpty")}
        </p>
      ) : (
        <ul className="luna-topic-claims__list">
          {list.claims.map((claim) => (
            <li
              key={claim.id}
              className="luna-topic-claims__item"
              data-conflict={claim.conflict || undefined}
            >
              {claim.conflict && (
                <span className="luna-topic-claims__tag">{t("topicSummary.conflictTag")}</span>
              )}
              {claim.transcript_record && (
                <span className="luna-topic-claims__tag">{t("topicSummary.transcriptTag")}</span>
              )}
              <span className="luna-topic-claims__text">{claim.text}</span>
              <span className="luna-topic-claims__sources">
                <CitationSources citations={claim.citations} onOpen={openCitation} />
                {claim.citations.length > 0 && (
                  <button
                    type="button"
                    className="luna-topic-claims__toggle"
                    aria-expanded={expanded.has(claim.id)}
                    onClick={() => toggleQuotes(claim.id)}
                  >
                    {t(expanded.has(claim.id) ? "topicSummary.hideQuotes" : "topicSummary.showQuotes")}
                  </button>
                )}
              </span>
              {claim.conflicts_with !== null && (
                <div className="luna-topic-claims__peer">
                  <span className="luna-topic-claims__tag">{t("topicSummary.conflictsWith")}</span>
                  <span className="luna-topic-claims__text">{claim.conflicts_with.text}</span>
                  <span className="luna-topic-claims__sources">
                    <CitationSources
                      citations={claim.conflicts_with.citations}
                      onOpen={openCitation}
                    />
                  </span>
                </div>
              )}
              {expanded.has(claim.id) && (
                <ul className="luna-topic-claims__quotes">
                  {[...claim.citations, ...(claim.conflicts_with?.citations ?? [])].map((citation) => (
                    <li key={citation.citation_id} className="luna-topic-claims__quote">
                      <span className="luna-topic-claims__quote-label">
                        {t("topicSummary.quoteFrom", {
                          title: citation.source_title ?? t("topicSummary.unresolvedSource"),
                        })}
                      </span>
                      <q className="luna-topic-claims__quote-text">{citation.quote}</q>
                    </li>
                  ))}
                </ul>
              )}
            </li>
          ))}
        </ul>
      )}
      {list !== undefined && list !== null && list.omitted > 0 && (
        <p className="luna-topic-summary__muted">
          {t("topicSummary.filterOmitted", { count: list.omitted })}
        </p>
      )}
    </section>
  );
}

/** 一条声明的出处行：来源（可点开）、此刻状态、来源「录于何时」（没记就不显示）。 */
function CitationSources({
  citations,
  onOpen,
}: {
  citations: TopicClaimCitationDto[];
  onOpen: (citation: TopicClaimCitationDto) => void;
}) {
  const { t, i18n } = useTranslation("workspace");
  return citations.map((citation) => (
    <span key={citation.citation_id} className="luna-topic-claims__source">
      <button
        type="button"
        className="luna-topic-claims__source-link"
        disabled={citation.source_path === null}
        onClick={() => onOpen(citation)}
      >
        {citation.source_title ?? t("topicSummary.unresolvedSource")}
      </button>
      <span className="luna-topic-claims__state" data-state={citation.state}>
        {t(`topicSummary.citationState.${citation.state}`)}
      </span>
      {citation.recorded_ms !== null && (
        <span className="luna-topic-claims__recorded">
          {t("topicSummary.recordedOn", { date: formatDate(citation.recorded_ms, i18n.language) })}
        </span>
      )}
    </span>
  ));
}
