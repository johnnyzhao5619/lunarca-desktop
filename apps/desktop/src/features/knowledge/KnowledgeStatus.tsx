/**
 * 「知识库 → 概览」上半部的**现状**（PRD-07-024a ①）。
 *
 * 这一格此前只有一条建议横幅加一张导航卡的拷贝——既不展示状态，也不回答问题。现状块
 * 回答的是「我的知识库现在什么样」：规模、以及最近在写什么。
 *
 * **安静的日子整块不渲染**（PRD-07-024a ①）：库里一篇都没有时不摆一排零。一个恒显
 * 却经常全是零的区块，会被用户学会跳过——那比不做更糟。
 *
 * **只回答「现在什么样」，不掺任何建议**：下一步归 `KnowledgePulse` 与维护区，两处
 * 排序不得在这里出现第三份（同 `knowledge_overview` 命令的注释）。
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type KnowledgeOverviewDto } from "../../ipc/bindings";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { SparklesIcon } from "../../components/ui/Icon";
import { requestDistillBatch } from "../distill/distill-actions";
import { goTo, openNote } from "../../lib/navigate";

/**
 * 知识库现状的取数（工作台与现状块共用一次：工作台据它判断「空库」换成引导空态，现状块据它
 * 画规模——两处各取一遍就是同一个数字两个来源）。`loaded` 区分「还没回来」与「取到了」：
 * 还没回来时不能当空库，否则每次打开都先闪一下引导。
 */
export function useKnowledgeOverview(): { status: KnowledgeOverviewDto | null; loaded: boolean } {
  const [state, setState] = useState<{ status: KnowledgeOverviewDto | null; loaded: boolean }>({
    status: null,
    loaded: false,
  });
  /** 最近一次 `load`（`useVaultReload` 恒调最新那一份）。 */
  const loadRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    let disposed = false;
    const load = () => {
      // 时间基准由前端给：Rust 侧没有「用户此刻的本地时间」这个事实，而窗口两端必须与
      // 用户看到的日期同一口径。
      void commands.knowledgeOverview(Date.now()).then((result) => {
        if (!disposed) {
          setState({ status: result.status === "ok" ? result.data : null, loaded: true });
        }
      });
    };
    load();
    loadRef.current = load;
    return () => {
      disposed = true;
      loadRef.current = null;
    };
  }, []);

  // 笔记增删改都会改变这两个数字；与其余清单同用一条既有订阅（合并同一阵变更），
  // 不新造刷新通路。
  useVaultReload(() => loadRef.current?.());
  return state;
}

/** 库里一篇知识笔记、一份来源都没有（知识库无从长起）。 */
export function isKnowledgeEmpty(status: KnowledgeOverviewDto): boolean {
  return status.knowledge_notes === 0 && status.materials === 0;
}

export function KnowledgeStatus({
  status,
  /**
   * 推荐卡此刻的主动作就是「整理现有资料」时，这颗同名手动入口让位（判定由
   * `KnowledgeOverview` 依 Rust 回传的动作做）：同一个动作同屏两枚，用户第一件事
   * 就是猜它们是不是同一件事。与脉搏时代「主动提醒 vs 主动发起」的分工不冲突——
   * 那条分工分的是两句**不同的话**，这里挡的是**同一颗按钮**出现两次。
   */
  hideTidy = false,
}: {
  /** `useKnowledgeOverview` 取到的现状；取数失败为 `null`。 */
  status: KnowledgeOverviewDto | null;
  hideTidy?: boolean;
}) {
  const { t } = useTranslation("workspace");

  // 取数失败或空库：整块不渲染。现状是增值信息，出不来不该占一块骨架屏。
  if (status === null || isKnowledgeEmpty(status)) {
    return null;
  }

  return (
    <section className="luna-knowledge-status" aria-label={t("knowledge.status.label")}>
      <p className="luna-knowledge-status__scale">
        {t("knowledge.status.scale", {
          notes: status.knowledge_notes,
          materials: status.materials,
        })}
        {/* **动机与动作同处一屏**（PRD-16-039 2026-08-30 修订）：整理资料是流水线上
            「资料 → 知识」那一步，用户想做它时会去看自己有多少资料——那个数字就在这句话
            里。它此前藏在图谱侧栏一个默认收起的折叠区里，等于不存在。
            **只在真有资料时出现**：一个库里一份资料都没有时，「整理现有资料」按下去
            只会得到一张说「无需整理」的确认卡——那是把用户支使一趟。
            与推荐卡「待整理」档的分工：一个是系统主动提醒（有活儿要干），一个是用户
            主动发起（我现在就要跑）——但推荐卡的主动作恰好也是它时本颗让位
            （`hideTidy`，判定在 `KnowledgeOverview`，见 PRD-16-039 修订注）。
            文案键仍用 `graph.nextAction.maintenanceTidy`——同一个动作只该有一个名字键，
            搬了位置不是改名的理由（i18n 术语守护同一条纪律）。 */}
        {status.materials > 0 && !hideTidy && (
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-btn--sm luna-knowledge-status__tidy"
            onClick={() => requestDistillBatch(null)}
          >
            <SparklesIcon size={13} />
            <span>{t("graph.nextAction.maintenanceTidy")}</span>
          </button>
        )}
      </p>
      {/* **只在真有孤立笔记时出现**：0 篇时说「0 篇还没有关系」是一句正确但没用的话，
          而现状块的纪律是没变化就安静（PRD-07-024a ①）。这个数与体检的孤儿页同源同数
          （判据在 Rust 的 `classify_connectivity`，两处共用）。 */}
      {status.unlinked_notes > 0 && (
        <p className="luna-knowledge-status__unlinked">
          {t("knowledge.status.unlinked", { count: status.unlinked_notes })}
        </p>
      )}
      {/* 一条值得回看的旧笔记（PRD-07-024a ①）：点它直接打开。Rust 保证它确实够旧
          （30 天没动过），够不上就返回 null——现状块宁可少一行，也不端一条昨天写的
          笔记出来说「值得回看」。 */}
      {status.revisit && (
        <p className="luna-knowledge-status__revisit">
          <span className="luna-knowledge-status__tags-label">
            {t("knowledge.status.revisit")}
          </span>
          <button
            type="button"
            className="luna-knowledge-status__tag"
            onClick={() => {
              openNote(status.revisit?.path ?? "");
            }}
          >
            {status.revisit.title}
          </button>
        </p>
      )}
      {status.active_tags.length > 0 && (
        <p className="luna-knowledge-status__tags">
          <span className="luna-knowledge-status__tags-label">
            {t("knowledge.status.activeTags")}
          </span>
          {status.active_tags.map((entry) => (
            <button
              key={entry.tag}
              type="button"
              className="luna-knowledge-status__tag"
              onClick={() => {
                goTo(`tags://${encodeURIComponent(entry.tag)}`);
              }}
            >
              #{entry.tag}
            </button>
          ))}
        </p>
      )}
    </section>
  );
}
