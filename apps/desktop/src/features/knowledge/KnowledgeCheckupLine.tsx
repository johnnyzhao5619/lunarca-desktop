/**
 * 体检一行（PRD-16-030：有发现才出现），住启动概览与「知识库 → 主题页工作台」两处。
 *
 * 刻意不并进知识库脉搏的优先级栈（PRD-16-027 / PRD-16-039）：栈回答的是「此刻最该做的
 * 一件事」，顺序是流水线的先后；体检回答的是另一个问题——「库本身有没有该收拾的地方」。
 * 塞进栈顶会顶掉真正的下一步，塞进栈底则永远轮不到它。故它是独立的一行，且没有发现时
 * 整行不渲染（零打扰）。
 *
 * 纯呈现：数字由宿主经 `useCheckupCount()` 取好传进来。两个宿主对「什么时候画」的要求
 * 不同——概览要等三条来源到齐再一起画，工作台可以到了就画——那是宿主的事；组件自己
 * 取数的话，这两种时序里必然有一种做不到。
 *
 * 语气由宿主给：概览上它排在装机与脉搏之后，那两条有话说时它让出主按钮
 * （`features/dashboard/next-actions.ts`）；工作台上主动作归推荐卡，它恒为 muted。
 */

import { useTranslation } from "react-i18next";
import { StethoscopeIcon } from "@lunarca/editor-core/Icon";
import { requestKnowledgeCheckup } from "../../lib/app-events";
import type { NextActionTone } from "../../lib/next-action-tone";

export function KnowledgeCheckupLine({
  count,
  tone = "accent",
  onDismiss,
}: {
  count: number;
  tone?: NextActionTone;
  onDismiss?: () => void;
}) {
  const { t } = useTranslation("workspace");
  if (count === 0) {
    return null;
  }
  return (
    <section className="luna-next-row" data-tone={tone} aria-label={t("checkup.title")}>
      <div className="luna-next-row__main">
        <StethoscopeIcon size={16} />
        <p>{t("checkup.dashboardLine", { count })}</p>
      </div>
      <div className="luna-next-row__actions">
        <button
          type="button"
          className={
            tone === "accent" ? "luna-btn luna-btn--primary" : "luna-btn luna-btn--secondary"
          }
          onClick={() => {
            onDismiss?.();
            requestKnowledgeCheckup();
          }}
        >
          {t("checkup.dashboardOpen")}
        </button>
      </div>
    </section>
  );
}
