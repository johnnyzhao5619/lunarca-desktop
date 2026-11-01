/**
 * 知识库脉搏行（PRD-07-057 / PRD-16-027）。
 *
 * 组织原则「采集 → 资料 → 蒸馏 → 知识 → 复利」的弹窗化身：任何时刻只渲染优先级栈的
 * 栈顶一态 + 一个主按钮，绝不并排三个按钮让用户选。判定见 `knowledge-pulse.ts`，
 * 取数见 `use-knowledge-pulse.ts`；本组件只呈现。
 *
 * 只住启动概览一个家（2026-08-30 收口，PRD-16-027 修订）：「知识库 → 概览」曾同时叠着
 * 脉搏与推荐卡，两套引擎同屏必然自相矛盾——那一侧改由 `KnowledgeNextActionCard` 单引擎
 * 承载。曝光表面因此是常量；若再添宿主，surface 必须随宿主重新参数化。
 *
 * `tone` 由概览统一裁决（`next-actions.ts`）而不是本组件自己主张：装机那一条更靠前时，
 * 脉搏要让出主按钮。主题态（栈顶与并列行）都渲染 `TopicSuggestionRow`，句子、三个动作、
 * 合成进行态由那一份代码保证（PRD-16-016）；栈顶那份只多一个 `primary`。
 */

import { useTranslation } from "react-i18next";
import type { SuggestionSurfaceDto } from "../../ipc/bindings";
import { SparklesIcon } from "../../components/ui/Icon";
import { requestDistillBatch } from "../distill/distill-actions";
import { TopicSuggestionRow } from "../knowledge/TopicSuggestionRow";
import { goTo } from "../../lib/navigate";
import type { NextActionTone } from "../../lib/next-action-tone";
import type { KnowledgePulseHandle } from "./use-knowledge-pulse";

/** 脉搏唯一的家（见文件头）。 */
const SURFACE: SuggestionSurfaceDto = "startup_overview";

export function KnowledgePulse({
  pulse,
  tone,
  onDismiss,
}: {
  pulse: KnowledgePulseHandle;
  tone: NextActionTone;
  onDismiss?: () => void;
}) {
  const { t } = useTranslation("workspace");
  const { state, aside, reload } = pulse;

  // 取数失败/未返回：整块不渲染——脉搏是增值信息，出不来不该占一块骨架屏。
  if (state === null) {
    return null;
  }

  const accent = tone === "accent";
  const actionClass = accent ? "luna-btn luna-btn--primary" : "luna-btn luna-btn--secondary";

  const viewMaterial = () => {
    // 跨视图动线：关弹窗，落资料空间（ADR-087 ①）。
    onDismiss?.();
    goTo("recording://");
  };

  return (
    <section
      className="luna-next-row"
      data-tone={tone}
      aria-label={t("dashboard.knowledgePulse.label")}
    >
      {state.kind === "pending" && (
        <>
          <div className="luna-next-row__main">
            <SparklesIcon size={16} />
            <p>
              {t("dashboard.knowledgePulse.pending", { count: state.pending })}
              {state.edited > 0 && (
                <span className="luna-next-row__aside">
                  {t("dashboard.knowledgePulse.editedAside", { count: state.edited })}
                </span>
              )}
            </p>
          </div>
          <div className="luna-next-row__actions">
            <button type="button" className="luna-btn luna-btn--secondary" onClick={viewMaterial}>
              {t("dashboard.knowledgePulse.viewMaterial")}
            </button>
            <button
              type="button"
              className={actionClass}
              onClick={() => {
                // 关弹窗再拉确认卡（宿主常驻 WorkspaceShell，不受弹窗生命周期影响）。
                onDismiss?.();
                requestDistillBatch(null);
              }}
            >
              {t("dashboard.knowledgePulse.distill")}
            </button>
          </div>
        </>
      )}
      {state.kind === "topic" && (
        <TopicSuggestionRow
          suggestion={state.suggestion}
          more={state.more}
          primary={accent}
          surface={SURFACE}
          onDismiss={onDismiss}
          onSettled={reload}
        />
      )}
      {state.kind === "settled" && (
        <div className="luna-next-row__main">
          <SparklesIcon size={16} />
          <p>
            {t("dashboard.knowledgePulse.settled", { count: state.fresh + state.edited })}
            {state.edited > 0 && (
              <span className="luna-next-row__aside">
                {t("dashboard.knowledgePulse.editedAside", { count: state.edited })}
              </span>
            )}
          </p>
        </div>
      )}
      {state.kind === "empty" && (
        <div className="luna-next-row__main">
          <SparklesIcon size={16} />
          <p>{t("dashboard.knowledgePulse.empty")}</p>
        </div>
      )}
      {/* 主题页建议不进栈，另占并列的一行（判定与理由见 `pulseTopicSuggestion`）。 */}
      {aside ? (
        <div className="luna-knowledge-pulse__aside-row">
          <TopicSuggestionRow
            suggestion={aside}
            surface={SURFACE}
            onDismiss={onDismiss}
            onSettled={reload}
          />
        </div>
      ) : null}
    </section>
  );
}
