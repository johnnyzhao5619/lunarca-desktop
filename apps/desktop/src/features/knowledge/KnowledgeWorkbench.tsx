/**
 * 知识库工作台（PRD-07-024a 2026-08-31 修订）：「主题页」分区**未选中页时**的主区内容
 * ——原「概览」分区的三块原样迁入（现状 + 下一步引擎 + 体检行），分区本身已退役
 * （空侧栏 + 纯转发页 + 建议三处重复）。仍只**组合**已有知识块，不另造第二张图谱或
 * 第二套标签统计（ADR-087 ④：知识库是派生矩阵，不是第二存储目录）。
 *
 * **提取成组件的理由是「让位」需要一个共同宿主**：推荐卡此刻的主动作由 Rust 决定并
 * 回传（`onAction`），现状行里与它同名的手动入口要据此让位——同一个动作同屏两枚，
 * 用户第一件事就是猜它们是不是同一件事（真机第四轮在图谱侧栏坐实的形态，概览的
 * 「整理现有资料」与 `PendingDistill` 档是同一个坑）。兄弟组件之间传不了这件事，
 * 让 WorkspaceApp 拿着又是给壳层加一份不属于它的状态。
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { EmptyState } from "../../components/ui/EmptyState";
import { IndexingNotice } from "../../components/ui/IndexingNotice";
import { NotebookIcon, SparklesIcon } from "@lunarca/editor-core/Icon";
import { useIndexStatus } from "../../hooks/use-index-status";
import type { KnowledgeNextActionCommand } from "../../ipc/bindings";
import { KnowledgeCheckupLine } from "./KnowledgeCheckupLine";
import { useCheckupCount } from "./use-checkup-count";
import { KnowledgeNextActionCard } from "./KnowledgeNextActionCard";
import { KnowledgeRecentActivity } from "./KnowledgeRecentActivity";
import { KnowledgeReviewInbox } from "./KnowledgeReviewInbox";
import { isKnowledgeEmpty, KnowledgeStatus, useKnowledgeOverview } from "./KnowledgeStatus";

export function KnowledgeWorkbench({ onCreateNote }: { onCreateNote: () => void }) {
  const { t } = useTranslation("workspace");
  const [action, setAction] = useState<KnowledgeNextActionCommand | null>(null);
  const checkup = useCheckupCount();
  const overview = useKnowledgeOverview();
  const indexing = useIndexStatus();
  // 空库（一篇知识笔记、一份来源都没有）：下面各块全都无话可说，此前主区只剩推荐卡一句
  // 「暂时不用整理。」——像是整理完了，其实是还没开始（真机 2026-10-06）。换成引导空态：
  // 说清知识库从哪来，给写第一篇的入口；与录音 / 转写空库主区同一形态。扫描中不算空。
  if (overview.loaded && overview.status !== null && isKnowledgeEmpty(overview.status) && !indexing) {
    return (
      <section className="luna-knowledge-workbench">
        <EmptyState
          icon={<SparklesIcon size={24} />}
          title={t("knowledge.landing.title")}
          description={t("knowledge.landing.body")}
          action={
            <button type="button" className="luna-btn luna-btn--primary" onClick={onCreateNote}>
              <NotebookIcon size={14} />
              <span>{t("create.newNote")}</span>
            </button>
          }
        />
      </section>
    );
  }
  return (
    <section className="luna-knowledge-workbench">
      {/* 开库那一轮扫描期间下面各块读的是半截索引（V1b）：一句话说在最上面，不逐块重复。 */}
      <IndexingNotice />
      {/* 阅读入口先于维护；事实与主动作仍来自既有 Rust 契约。 */}
      <KnowledgeRecentActivity />
      {/* **概览只有一个下一步引擎**（PRD-16-027 2026-08-30 收口）：此前这里还叠着
          一块 `KnowledgePulse`，真机坐实了两套引擎同屏的矛盾——脉搏说「有 7 篇都在讲
          『数据/流向』」、紧挨着推荐卡说「暂时不用整理」。脉搏退居启动概览弹窗（它的
          出生地），主题页机会由本卡的 `TopicOpportunity` 档承载。
          与图谱维护区**共用同一个组件**（PRD-16-039 2026-08-30 修订）：概览这一格
          拿 scope="overview"，图谱拿 "graph"，分流判据在 Rust。两处长得一样不是巧合
          ——「同一份状态、同一套动作语义」由共用组件结构保证。 */}
      <KnowledgeNextActionCard scope="overview" onAction={setAction} />
      {/* 待你看（PRD-16-059）：后台备好的整理建议按主题分组；按钮都是次要样式——这一屏的
          主动作仍只有推荐卡那一个。没有待确认内容时整块不出现。 */}
      <KnowledgeReviewInbox />
      <KnowledgeStatus status={overview.status} hideTidy={action === "distill_batch"} />
      {/* 体检行是**并列的次要项**：主动作只有推荐卡那一个（PRD-16-053「最多一个推荐主动作」），
          故语气恒为 muted；推荐卡此刻推的正是体检时整行让位——同一个动作同屏两枚。 */}
      {action !== "open_checkup" && <KnowledgeCheckupLine count={checkup.count} tone="muted" />}
      {/* **导航卡与知识地图入口都已退役**（PRD-07-024a ③ / ADR-096 ② 2026-08-31 修订）：
          导航卡是左侧二级分区导航的第二份拷贝；地图入口按钮则是「主题页目录的文件形态」
          的第二个门——目录本身就是活的地图（同源同字节），地图现在钉在主题页分区目录
          首行，与其他产物同待遇。 */}
    </section>
  );
}
