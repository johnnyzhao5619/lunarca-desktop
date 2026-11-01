/**
 * 「下一步做什么」的推荐卡——**图谱维护区与知识库概览共用一份**。
 *
 * **提取的理由不是代码整洁，是同一件事必须长得一样**：三个建议表面此前形态各异
 * （横幅 / 空态句子 / 维护卡），用户第一步要做的判断不是「要不要做」，而是「这三个是不是
 * 同一件事」。共用组件让「同一份状态、同一套动作语义」成为结构保证（PRD-16-016 落地注
 * 明写「共享的是状态与动作语义，不是把同一排按钮原样复制三遍」——密度可以不同，语义不能）。
 *
 * **本组件不含「运行中」那一档**：重扫进度与取消是图谱侧栏自己的状态（它订阅
 * `graph://rescan`），留在那里。本组件只管「此刻该推哪一件」。
 *
 * **动作一律走全局派发器**（`requestGraphRescan` / `requestKnowledgeCheckup` /
 * `requestDistillBatch`）：确认卡与进行中状态由 WorkspaceShell 的唯一宿主承载，派发方
 * 不必知道卡在哪儿。这也是本组件能脱离图谱控制器的原因。
 *
 * 样式归 `knowledge.css` 的 `.luna-next-action*`（随组件两处安家而迁入本 feature，
 * TDD-15 §2.3 co-location）；图谱侧栏的「运行中」块占用同一卡位，共用同一套类。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  type KnowledgeActionScope,
  type KnowledgeNextActionCommand,
  type KnowledgeNextActionDto,
} from "../../ipc/bindings";
import { requestDistillBatch } from "../distill/distill-actions";
import { requestKnowledgeCheckup } from "../../lib/app-events";
import { requestGraphRescan } from "./graph-scope-menu";
import { TopicSuggestionRow } from "./TopicSuggestionRow";
import { useSuggestionVisibility } from "./use-suggestion-visibility";
import { useTopicDraftsSettled } from "./use-topic-drafts-settled";
import { TOPIC_DIRECTORY_STALE_EVENT } from "./topic-events";
import { vaultErrorText } from "../../lib/vault-error";
import { goTo } from "../../lib/navigate";
import { useVaultReload } from "../../hooks/use-vault-reload";

/** 地图差异预览最多渲染的改动行数：这是一张确认卡，不是 diff 阅读器——超过这个数
    说明该去版本历史看全量，而不是让卡片长到吞掉整个侧栏。 */
const MAP_REVIEW_DIFF_LINE_CAP = 80;

export function KnowledgeNextActionCard({
  scope,
  /** 外部触发重取（如图谱的重扫刚跑完）。 */
  reloadKey = 0,
  onReloaded,
  onAction,
}: {
  scope: KnowledgeActionScope;
  reloadKey?: number;
  onReloaded?: () => void;
  /**
   * 此刻推荐的主动作（`null` = 没有推荐）。宿主用它避开**同一个动作同屏两枚**：
   * 图谱侧栏的「更多维护」列着与推荐卡 CTA 相同的手动入口（真机第四轮：两颗
   * 「让更多笔记建立关系」同屏），推荐在场时手动那颗让位。判定内容仍全在 Rust
   * ——这里回传的只是 Rust 已做的决定，宿主不据 kind 自行推导第二遍。
   */
  onAction?: (action: KnowledgeNextActionCommand | null) => void;
}) {
  const { t } = useTranslation("workspace");
  const [nextAction, setNextAction] = useState<KnowledgeNextActionDto | null>(null);
  /** 取数三态：「还在查」「查不到」与「查到了、没有要做的事」是三句不同的话——此前前两者
      都落在「暂时不用整理」上，等于在出错时告诉用户一切安好。 */
  const [load, setLoad] = useState<"loading" | "ready" | "error">("loading");
  /** 动作本身失败时的原因（已本地化），就地给出，不吞掉。 */
  const [actionError, setActionError] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  const [mapResolverOpen, setMapResolverOpen] = useState(false);
  const [mapReviewOpen, setMapReviewOpen] = useState(false);
  const [mapReviewError, setMapReviewError] = useState<string | null>(null);
  const [mapToKeep, setMapToKeep] = useState<string | null>(null);
  const setNextActionReload = (fn: (n: number) => number) => {
    setReload(fn);
    onReloaded?.();
  };
  // 后台初稿备好、或在对话框里被放弃，推荐卡上的主题建议都要跟着换句子；两者都不改库文件。
  useTopicDraftsSettled(() => setReload((n) => n + 1));
  // 库变了（含开库那一轮扫描做完，V1b）就重取：推荐判的是索引上的数，扫描期间取到的是半截
  // （真机 2026-10-05：扫描做完后「有 12432 项发现」一直停着，实际是三万多项）。
  useVaultReload(() => setReload((n) => n + 1));
  useEffect(() => {
    const onStale = () => setReload((n) => n + 1);
    window.addEventListener(TOPIC_DIRECTORY_STALE_EVENT, onStale);
    return () => window.removeEventListener(TOPIC_DIRECTORY_STALE_EVENT, onStale);
  }, []);
  const nextActionVisibilityRef = useSuggestionVisibility(
    nextAction?.kind === "topic_opportunity" || nextAction?.kind === "knowledge_map_review"
      ? nextAction.suggestion_token
      : null,
    scope === "graph" ? "graph" : "knowledge_overview",
  );

  useEffect(() => {
    let disposed = false;
    void commands.knowledgeNextAction(scope).then((result) => {
      if (disposed) return;
      const action = result.status === "ok" ? result.data : null;
      setNextAction(action);
      setLoad(result.status === "ok" ? "ready" : "error");
      onAction?.(action?.action ?? null);
    });
    return () => {
      disposed = true;
    };
    // onAction 是宿主的稳定回调，刻意不进依赖：以它为依赖会让内联箭头函数每次渲染都触发重取。
  }, [scope, reload, reloadKey]);

const nextActionCopy = nextAction
  ? {
      topic_opportunity: [
        t("graph.nextAction.topicTitle"),
        t("graph.nextAction.topicDesc", { count: nextAction.count }),
      ],
      knowledge_map_missing: [
        t("graph.nextAction.mapMissingTitle"),
        t("graph.nextAction.mapMissingDesc", { count: nextAction.count }),
      ],
      knowledge_map_conflict: [
        t("graph.nextAction.mapConflictTitle"),
        t("graph.nextAction.mapConflictDesc", { count: nextAction.count }),
      ],
      knowledge_map_review: [
        t("graph.nextAction.mapReviewTitle"),
        t("graph.nextAction.mapReviewDesc", { count: nextAction.count }),
      ],
      checkup_finding: [
        t("graph.nextAction.checkupTitle"),
        t("graph.nextAction.checkupDesc", { count: nextAction.count }),
      ],
      tag_language_mismatch: [
        t("graph.nextAction.tagLanguageTitle"),
        t("graph.nextAction.tagLanguageDesc", { count: nextAction.count }),
      ],
      missing_tags: [
        t("graph.nextAction.tagsTitle"),
        t("graph.nextAction.tagsDesc", { count: nextAction.count }),
      ],
      pending_distill: [
        t("graph.nextAction.distillTitle"),
        t("graph.nextAction.distillDesc", { count: nextAction.count }),
      ],
    }[nextAction.kind]
  : null;

  if (load === "loading") {
    return (
      <p className="luna-next-action__desc" role="status">
        {t("common:status.loading")}
      </p>
    );
  }
  if (load === "error") {
    return (
      <div className="luna-next-action">
        <p className="luna-next-action__desc" role="alert">
          {t("graph.nextAction.loadError")}
        </p>
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-next-action__cta"
          onClick={() => setNextActionReload((n) => n + 1)}
        >
          {t("common:action.retry")}
        </button>
      </div>
    );
  }
  if (!nextAction || !nextActionCopy) {
    return <p className="luna-next-action__desc">{t("graph.nextAction.empty")}</p>;
  }

  return (
      <div
        ref={nextActionVisibilityRef}
        className="luna-next-action"
        data-suggestion-token={nextAction.suggestion_token}
      >
        <strong>{nextActionCopy[0]}</strong>
        {/* 主题机会的事实句由下面的建议行说（含篇数**与标签名**）；再渲染一遍
            「发现 N 篇共享主题的笔记」是同一句话的低信息版本，同屏两遍。 */}
        {nextAction.action === "create_topic" && nextAction.topic ? null : (
          <p className="luna-next-action__desc">{nextActionCopy[1]}</p>
        )}
        {/* 标签语言分裂：直接列出出现最多的几条「另一种语言」的标签（Rust 排好序，C16）——
            用户认得出 `#architecture` 与 `#架构`，比读一段「概念裂成两个标签」的解释快。 */}
        {nextAction.tags.length > 0 ? (
          <ul className="luna-next-action__tags" aria-label={t("graph.nextAction.tagLanguageSample")}>
            {nextAction.tags.map((tag) => (
              <li key={tag}>#{tag}</li>
            ))}
          </ul>
        ) : null}
        {nextAction.action === "create_topic" && nextAction.topic ? (
          <TopicSuggestionRow
            suggestion={nextAction.topic}
            // 表面随宿主走（度量要答得出「在哪儿看见、在哪儿动手」）；此前写死 "graph"
            // 是因为主题机会只可能出现在图谱格——概览单引擎化之后它主要落概览。
            surface={scope === "graph" ? "graph" : "knowledge_overview"}
            onSettled={() => setNextActionReload((n) => n + 1)}
            // 推荐卡的主动作是这一屏唯一的实色主按钮（PRD-16-056），主题机会也不例外——
            // 此前只有其余各档用实色，主题机会这一档整屏没有主按钮（真机 2026-10-01）。
            primary
          />
        ) : (
          <button
            type="button"
            // 这是这一屏唯一的推荐主动作（PRD-16-039 / PRD-16-053），用实色主按钮。
            className="luna-btn luna-btn--primary luna-next-action__cta"
            data-primary-action={nextAction.action}
            onClick={() => {
              setActionError(null);
              if (nextAction.action === "open_checkup") requestKnowledgeCheckup();
              if (nextAction.action === "rescan_tags") requestGraphRescan(null);
              // 语言分裂那一档必须直通「全部重新生成」：不一致的笔记指纹是最新的，
              // 「只补缺口」按指纹跳过、一篇都扫不到（PRD-06-035d ②）。
              if (nextAction.action === "rescan_tags_force") requestGraphRescan(null, "force");
              if (nextAction.action === "distill_batch") requestDistillBatch(null);
              if (nextAction.action === "create_knowledge_map") {
                void commands.knowledgeMapCreate().then((result) => {
                  if (result.status === "error") {
                    setActionError(vaultErrorText(result.error));
                    return;
                  }
                  setNextActionReload((n) => n + 1);
                  // 建好了就带用户去看它：点了「建立」却什么都没出现，读起来就是没反应。
                  if (result.data) goTo(`topics://${result.data}`);
                });
              }
              if (nextAction.action === "resolve_knowledge_map_conflict") {
                setMapResolverOpen(true);
                setMapToKeep(nextAction.maps[0]?.path ?? null);
              }
              if (nextAction.action === "review_knowledge_map") {
                setMapReviewError(null);
                setMapReviewOpen(true);
              }
              if (nextAction.action === "resolve_ai_readiness" && nextAction.target) {
                goTo(nextAction.target);
              }
            }}
          >
            {nextAction.action === "resolve_ai_readiness"
              ? t("graph.nextAction.resolveReadiness")
              : nextAction.action === "create_knowledge_map"
                ? t("graph.nextAction.createMap")
                : nextAction.action === "resolve_knowledge_map_conflict"
                  ? t("graph.nextAction.chooseMap")
                  : nextAction.action === "review_knowledge_map"
                    ? t("graph.nextAction.reviewMap")
                    : nextAction.action === "open_checkup"
                      ? t("graph.nextAction.maintenanceCheck")
                      : nextAction.action === "rescan_tags"
                        ? t("graph.nextAction.maintenanceConnect")
                        : nextAction.action === "rescan_tags_force"
                          ? t("graph.nextAction.tagLanguageCta")
                          : t("graph.nextAction.maintenanceTidy")}
          </button>
        )}
        {actionError ? (
          <p className="luna-field-error" role="alert">
            {actionError}
          </p>
        ) : null}
        {mapReviewOpen &&
        nextAction.action === "review_knowledge_map" &&
        nextAction.map_review ? (
          <div className="luna-next-action__map-review">
            <div
              className="luna-next-action__map-diff"
              aria-label={t("graph.nextAction.mapDiff")}
            >
              {nextAction.map_review.diff.lines
                .filter((line) => line.kind !== "equal")
                .slice(0, MAP_REVIEW_DIFF_LINE_CAP)
                .map((line, index) => (
                  <code
                    key={`${line.kind}-${line.old_line}-${line.new_line}-${index}`}
                    data-kind={line.kind}
                  >
                    {line.kind === "added" ? "+ " : "− "}
                    {line.text}
                  </code>
                ))}
            </div>
            {mapReviewError ? (
              <p className="luna-field-error" role="alert">
                {mapReviewError}
              </p>
            ) : null}
            <div className="luna-next-action__map-review-actions">
              <button
                type="button"
                className="luna-btn luna-btn--primary"
                onClick={() => {
                  const review = nextAction.map_review;
                  if (!review) return;
                  void commands
                    .knowledgeMapApplyReview(
                      review.path,
                      review.current_hash,
                      nextAction.suggestion_token,
                    )
                    .then((result) => {
                      if (result.status === "error") {
                        setMapReviewError(vaultErrorText(result.error));
                        return;
                      }
                      setMapReviewOpen(false);
                      setNextActionReload((n) => n + 1);
                    });
                }}
              >
                {t("graph.nextAction.applyMapReview")}
              </button>
              <button
                type="button"
                className="luna-btn luna-btn--secondary"
                onClick={() => setMapReviewOpen(false)}
              >
                {t("graph.nextAction.cancelMapReview")}
              </button>
            </div>
          </div>
        ) : null}
        {mapResolverOpen && nextAction.action === "resolve_knowledge_map_conflict" ? (
          <div className="luna-next-action__map-resolver">
            {nextAction.maps.map((map) => (
              <label key={map.path}>
                <input
                  type="radio"
                  name="knowledge-map"
                  checked={mapToKeep === map.path}
                  onChange={() => setMapToKeep(map.path)}
                />
                <span>{map.path}</span>
              </label>
            ))}
            <button
              type="button"
              className="luna-btn luna-btn--primary"
              disabled={mapToKeep === null}
              onClick={() => {
                if (!mapToKeep) return;
                void commands.knowledgeMapResolveConflict(mapToKeep).then((result) => {
                  if (result.status === "error") {
                    setActionError(vaultErrorText(result.error));
                    return;
                  }
                  setMapResolverOpen(false);
                  setNextActionReload((n) => n + 1);
                });
              }}
            >
              {t("graph.nextAction.keepMap")}
            </button>
          </div>
        ) : null}
      </div>
  );
}
