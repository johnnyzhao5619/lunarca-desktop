/**
 * 主题页建议行（PRD-16-016 的 aha 时刻）——**这条建议在哪儿出现，用的都是这一份**。
 *
 * **它为什么要独立成组件**：主题页最初**唯一的出生入口**只活在启动概览弹窗里
 * （PRD-07-057 二次修订：弹窗每次运行只呈现一次），关掉就再也回不去——而"库里聚了一簇
 * 同标签的笔记"这件事是持续发生的，它的入口不能是一次性的（2026-07-30 真机上报）。
 * 现在的表面清单（PRD-16-016 落地注）：启动概览弹窗（栈顶态与并列行都是本组件）、
 * 「知识库 → 概览」的推荐卡（`KnowledgeNextActionCard` 的 `TopicOpportunity` 档）、
 * 「知识库 → 主题页」分区的 0 页空态。三处共享的是**状态与动作语义**——句子、
 * 三个动作（看看是哪几篇 / 合成 / ✕）、合成进行态，全由这一份代码保证。
 *
 * **谁来决定摆哪一条建议由宿主定**（本组件只收一条）：启动概览要避开优先级栈的栈顶
 * （`pulseTopicSuggestion`），推荐卡由 `knowledge_next_action` 给。判据全在 Rust
 * （`Vault::topic_suggestions`：同标签 ≥3 篇知识笔记、尚无主题页以它为出身、
 * 不在忽略清单），前端零业务（ADR-003）。
 *
 * 后台已备好初稿时（`draft_items`，PRD-16-060）句子后面跟一句「初稿已备好 · N 条」，主按钮改叫
 * 「查看初稿」——点开是那份封存初稿，不再等模型；仍要用户确认才建页。
 */

import { useTranslation } from "react-i18next";
import type { SuggestionSurfaceDto, TopicSuggestionDto } from "../../ipc/bindings";
import { requestTopicDraft } from "./topic-draft";
import { TopicIgnoreButton } from "./TopicIgnoreButton";
import { useSuggestionVisibility } from "./use-suggestion-visibility";
import { goTo } from "../../lib/navigate";

interface TopicSuggestionRowProps {
  suggestion: TopicSuggestionDto;
  /** 这一行"用完了"（已入队合成 / 已被忽略）：宿主据此重取或让路。 */
  onSettled: () => void;
  /** 弹窗宿主用：入队成功后关掉自己（常驻宿主不传——它没什么要关）。 */
  onDismiss?: () => void;
  /**
   * 曝光上报的表面；`null` = **这不是一枚主动提醒**（「知识库 → 主题页」分区的
   * 0 页空态，PRD-16-016 / TDD-07 §4.1c）：既不上报曝光，**也不承载 ✕**。
   *
   * ✕ 消掉的只是主动冒出来的提醒（启动概览 / 概览推荐卡）；分区空态是用户主动走进来
   * 才看见的解释，恒显——把它也做成可关掉的，等于让那个分区从此答不出「这里为什么
   * 是空的」。
   */
  surface: SuggestionSurfaceDto | null;
  /** 除本条外还有几个主题也够格（启动概览的栈顶态用；`0`/缺省 = 不提）。 */
  more?: number;
  /**
   * 「合成主题页」用主按钮层级。只有**这一屏唯一主动作**的宿主传它：启动概览弹窗的栈顶态
   * （PRD-16-027）与推荐卡（PRD-16-056）；主题页 0 页空态保持次级——那里它与周围动作并列。
   */
  primary?: boolean;
}

export function TopicSuggestionRow({
  suggestion,
  onSettled,
  onDismiss,
  surface,
  more = 0,
  primary = false,
}: TopicSuggestionRowProps) {
  const { t } = useTranslation("workspace");
  const visibilityRef = useSuggestionVisibility(suggestion.suggestion_token, surface);

  return (
    <div ref={visibilityRef} className="luna-topic-suggestion">
      <p className="luna-topic-suggestion__text">
        {t("dashboard.knowledgePulse.topic", {
          // 精确的知识笔记篇数由 Rust 给；`notes` 截在门槛四倍，长度不是篇数（PRD-16-051）。
          count: suggestion.note_count,
          tag: suggestion.tag,
        })}
        {suggestion.draft_items !== null && (
          <span className="luna-topic-suggestion__more">
            {t("dashboard.knowledgePulse.topicDraftReady", { count: suggestion.draft_items })}
          </span>
        )}
        {more > 0 && (
          <span className="luna-topic-suggestion__more">
            {t("dashboard.knowledgePulse.topicMore", { count: more })}
          </span>
        )}
      </p>
      <div className="luna-topic-suggestion__actions">
        {/* 「看看是哪几篇」：落到那个标签的视图——先看再决定，不逼着立刻建页。
            各表面同语义（此前只有启动概览的栈顶态有它，概览与主题页空态没有——
            同一条建议在不同表面给不同的决策余地，没道理）。 */}
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          onClick={() => {
            onDismiss?.();
            goTo(`tags://${encodeURIComponent(suggestion.tag)}`);
          }}
        >
          {t("dashboard.knowledgePulse.topicPeek")}
        </button>
        <button
            type="button"
            className={`luna-btn ${primary ? "luna-btn--primary" : "luna-btn--secondary"}`}
            data-primary-action="create_topic"
            onClick={() => {
              // 交给 `TopicDraftHost`：准备 → 逐条预览 → 确认后才写入（ADR-134）。
              requestTopicDraft(suggestion.tag);
              onDismiss?.();
              onSettled();
            }}
          >
            {t(
              suggestion.draft_items !== null
                ? "dashboard.knowledgePulse.topicReview"
                : "dashboard.knowledgePulse.topicCreate",
            )}
          </button>
        {/* ✕ 即对该标签永久安静（PRD-16-004）；下一条顺位由宿主重取补上。
            `surface === null` = 主题页 0 页空态：它不承载 ✕（PRD-16-016）。 */}
        {surface !== null && (
          <TopicIgnoreButton
            tag={suggestion.tag}
            suggestionToken={suggestion.suggestion_token}
            className="luna-topic-suggestion__ignore"
            onSettled={onSettled}
          />
        )}
      </div>
    </div>
  );
}
