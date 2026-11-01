/**
 * 知识库工作台的「继续阅读」与「近 7 天」（K5 / PRD-16-067 / TDD-07 §4.1e）。
 *
 * 两者都由 Rust `knowledge_recent_activity` 判定，前端只渲染：
 * - 继续阅读：阅读视图最近打开、且此刻仍是知识产物的那一页（记在库内 `.luna` 界面现场，
 *   冷启动后仍在；被删、被移走或转为普通笔记后不给）。
 * - 近 7 天（ADR-136 ⑦）：这段时间收进来多少来源（按录于时间）、主题页上新写入多少条结论、
 *   哪几页长得最多，以及收进来却还没被任何主题页引用的那几篇。滚动 7 天、确定性派生、
 *   不调用模型、不推送、不写进库。它取代了原来的「最近整理」列表——同一件事不列两遍。
 *
 * 两块都「有内容才显示」：空库、没读过、这 7 天什么都没发生时整块不渲染，不留空壳。
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type KnowledgeRecentActivityDto, type KnowledgeWeekDto } from "../../ipc/bindings";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { goTo, openNote } from "../../lib/navigate";

function weekIsEmpty(week: KnowledgeWeekDto): boolean {
  return week.notes + week.quick_notes + week.recordings + week.claims === 0;
}

export function KnowledgeRecentActivity() {
  const { t } = useTranslation("workspace");
  const [activity, setActivity] = useState<KnowledgeRecentActivityDto | null>(null);
  const requestRef = useRef(0);

  const load = () => {
    const request = ++requestRef.current;
    void commands.knowledgeRecentActivity().then((result) => {
      if (request === requestRef.current) {
        setActivity(result.status === "ok" ? result.data : null);
      }
    });
  };

  useEffect(() => {
    load();
    // 挂载时取一次；之后只跟着库变化重取。
  }, []);
  useVaultReload(load);

  if (activity === null || (activity.continue_reading === null && weekIsEmpty(activity.week))) {
    return null;
  }
  const { continue_reading: resume, week } = activity;
  return (
    <section className="luna-knowledge-recent" aria-label={t("knowledgeRecent.label")}>
      {resume !== null && (
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          onClick={() => goTo(`topics://${resume.path}`)}
        >
          {t("knowledgeRecent.continue", { title: resume.title })}
        </button>
      )}
      {!weekIsEmpty(week) && <WeekReview week={week} />}
    </section>
  );
}

function WeekReview({ week }: { week: KnowledgeWeekDto }) {
  const { t } = useTranslation("workspace");
  const sourceTotal = week.notes + week.quick_notes + week.recordings;
  const parts = [
    week.notes > 0 && t("knowledgeRecent.weekNotes", { count: week.notes }),
    week.quick_notes > 0 && t("knowledgeRecent.weekQuickNotes", { count: week.quick_notes }),
    week.recordings > 0 && t("knowledgeRecent.weekRecordings", { count: week.recordings }),
  ].filter((part): part is string => typeof part === "string");
  return (
    <>
      <h2 className="luna-knowledge-recent__title">{t("knowledgeRecent.weekTitle")}</h2>
      {sourceTotal > 0 && (
        <p className="luna-knowledge-recent__summary">
          {t("knowledgeRecent.weekSources", {
            count: sourceTotal,
            parts: parts.join(t("knowledgeRecent.partSeparator")),
          })}
        </p>
      )}
      {week.claims > 0 && (
        <p className="luna-knowledge-recent__summary">
          {week.conflicts > 0
            ? t("knowledgeRecent.weekClaimsWithConflicts", {
                count: week.claims,
                conflicts: week.conflicts,
              })
            : t("knowledgeRecent.weekClaims", { count: week.claims })}
        </p>
      )}
      {week.topics.length > 0 && (
        <ul className="luna-knowledge-recent__list">
          {week.topics.map((topic) => (
            <li key={topic.path} className="luna-knowledge-recent__item">
              <button
                type="button"
                className="luna-knowledge-recent__link"
                onClick={() => goTo(`topics://${topic.path}`)}
              >
                {topic.title}
              </button>
              <span className="luna-knowledge-recent__time">
                {topic.conflicts > 0
                  ? t("knowledgeRecent.weekTopicWithConflicts", {
                      count: topic.claims,
                      conflicts: topic.conflicts,
                    })
                  : t("knowledgeRecent.weekTopic", { count: topic.claims })}
              </span>
            </li>
          ))}
        </ul>
      )}
      {week.topics_omitted > 0 && (
        <p className="luna-knowledge-recent__summary">
          {t("knowledgeRecent.weekTopicsOmitted", { count: week.topics_omitted })}
        </p>
      )}
      {week.unfiled_count > 0 && (
        <>
          <h3 className="luna-knowledge-recent__title" title={t("knowledgeRecent.weekUnfiledHint")}>
            {t("knowledgeRecent.weekUnfiled", { count: week.unfiled_count })}
          </h3>
          <ul className="luna-knowledge-recent__list">
            {week.unfiled.map((source) => (
              <li key={source.path} className="luna-knowledge-recent__item">
                <button
                  type="button"
                  className="luna-knowledge-recent__link"
                  onClick={() => openNote(source.path)}
                >
                  {source.title}
                </button>
              </li>
            ))}
          </ul>
          {week.unfiled_count > week.unfiled.length && (
            <p className="luna-knowledge-recent__summary">
              {t("knowledgeRecent.weekUnfiledMore", {
                count: week.unfiled_count - week.unfiled.length,
              })}
            </p>
          )}
        </>
      )}
    </>
  );
}
