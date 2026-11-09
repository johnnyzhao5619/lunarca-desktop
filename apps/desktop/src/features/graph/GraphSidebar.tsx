/**
 * 关系图谱主侧栏（PRD-07-057 两层结构 / PRD-06-034）。
 *
 * 图谱此前把搜索、过滤器、聚焦芯片全塞在主区顶部的工具条里，而应用其它目的地一律是
 * 「侧栏承载控制、主区呈现内容」——图谱是唯一的例外（真机反馈「与其他功能页面统一
 * 风格和交互逻辑」）。归位后侧栏分区：搜索（页头，重新布局作页头动作——它是动作
 * 不是过滤器，混进过滤器区语义不对）/ 过滤器 / AI 关系标签（全库补齐入口 +
 * 进行中进度，PRD-06-035b/035c）。节点颜色直接长在对应开关上，避免另放一份重复图例。
 *
 * 状态经 `GraphController` 与主区画布共享单一实例（与搜索 / 设置同构）。
 *
 * 网络粒度（PRD-16-062）：「主题 / 笔记」两档，默认主题。主题网络里只有主题节点与共享来源
 * 边，标签、未创建、笔记类型与成熟度这几组过滤器无物可滤，故只在笔记网络里出现。
 */

import { useEffect, useRef, useState } from "react";
import { IconButton } from "../../components/ui/IconButton";
import { useTranslation } from "react-i18next";
import { KnowledgeNextActionCard } from "../knowledge/KnowledgeNextActionCard";
import { commands, events } from "../../ipc/bindings";
import {
  ArrowClockwiseIcon,
  CloseIcon,
  StethoscopeIcon,
  TagsIcon,
} from "@lunarca/editor-core/Icon";
import { QueryBox } from "../../components/ui/QueryBox";
import { requestKnowledgeCheckup } from "../../lib/app-events";
import { useProcessingQueueSelector } from "../processing/use-processing-queue";
import { isTaskActive } from "../processing/processing-labels";
import {
  MATURITY_FILTER_GROUP,
  NOTE_TYPE_FILTERS,
  NOTE_TYPE_GROUPS,
} from "./graph-sidebar-config";
import type { GraphController } from "./use-graph-controller";

export function GraphSidebar({ controller }: { controller: GraphController }) {
  const { t } = useTranslation("workspace");
  const {
    mode,
    setMode,
    filtered,
    query,
    setQuery,
    showTags,
    toggleTags,
    showGhosts,
    toggleGhosts,
    showOrphans,
    toggleOrphans,
    noteTypes,
    toggleNoteType,
    showDrafts,
    toggleDrafts,
    focus,
    setFocus,
    focusLabel,
    requestRelayout,
    openRescanDialog,
    rescanProgress,
    cancelRescan,
  } = controller;
  /** 维护位只消费 Rust 排好序的一条建议，绝不让四个异步 hook 在前端抢位。 */
  const [nextActionReload, setNextActionReload] = useState(0);
  /** 推荐卡此刻的主动作（Rust 决定，卡片回传）：与它相同的手动维护入口让位——
      真机第四轮坐实过两颗「让更多笔记建立关系」同屏。 */
  const [recommended, setRecommended] = useState<string | null>(null);
  const [showMaintenance, setShowMaintenance] = useState(false);
  const [checkupRunning, setCheckupRunning] = useState(false);
  const maintenanceToggleRef = useRef<HTMLButtonElement>(null);
  const rescanning = rescanProgress !== null;
  /** 待收拾的处数（口径与启动概览那一行同源：`backlog` 不计入）。`0` = 不渲染徽标。
      默认推荐位只推**一件**，而「有几处可以收拾」是持续为真的事实——主题机会压过
      体检时它就再也没有常驻去处了，故折叠区那个按钮上的徽标必须留着。 */
  const [hygiene, setHygiene] = useState(0);
  // 只取「有没有在跑的整理」这一位：队列进度一拍一推，取整份就是每一拍重渲染整个侧栏。
  const distilling = useProcessingQueueSelector((queue) =>
    queue.items.some((item) => item.kind === "distill" && isTaskActive(item)),
  );
  const activeMaintenance = rescanning
    ? t("graph.nextAction.runningTags")
    : checkupRunning
      ? t("graph.nextAction.runningCheckup")
      : distilling
        ? t("graph.nextAction.runningDistill")
        : null;
  useEffect(() => {
    const unlisten = events.checkupProgress.listen((event) => {
      setCheckupRunning(!event.payload.finished);
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, []);
  useEffect(() => {
    let disposed = false;
    // 推荐本身由 `KnowledgeNextActionCard` 自己取（scope="graph"，分流判据在 Rust）。
    // 这里只留体检徽标要的那一次——它是折叠区按钮上的常驻数字，与推荐位不是一回事。
    void commands.checkupReport().then((result) => {
      if (disposed || result.status !== "ok") return;
      const findings = result.data.findings.filter((finding) => !finding.backlog);
      setHygiene(findings.length + result.data.truncated);
    });
    return () => {
      disposed = true;
    };
  }, [nextActionReload, rescanning, checkupRunning, distilling]);


  return (
    <section className="luna-graph-side" aria-label={t("graph.title")}>
      <header className="luna-graph-side__header luna-sidebar-header">
        <div className="luna-graph-side__title-row">
          {/* 名字由二级分区导航给（PRD-07-065）。 */}
          <div className="luna-sidebar-header__title">
            <span className="luna-count-badge">
              {/* 第一次读取中不报「0 节点」：真机上那几秒像是图谱空了（K7）。 */}
              {controller.data === null && controller.error === null
                ? t("common:status.loading")
                : t("graph.stats", { nodes: filtered.nodes.length, edges: filtered.edges.length })}
            </span>
          </div>
          <IconButton
            label={t("graph.relayout")}
            icon={<ArrowClockwiseIcon size={14} />}
            onClick={requestRelayout}
          />
        </div>
        <QueryBox
          value={query}
          onChange={setQuery}
          placeholder={t(mode === "topics" ? "graph.searchTopics" : "graph.searchPlaceholder")}
          ariaLabel={t(mode === "topics" ? "graph.searchTopics" : "graph.searchPlaceholder")}
        />
      </header>

      <div className="luna-graph-side__body">
        <section className="luna-graph-side__section" aria-label={t("graph.mode")}>
          <h3 className="luna-graph-side__section-title">{t("graph.mode")}</h3>
          <div className="luna-graph-side__toggles" role="group" aria-label={t("graph.mode")}>
            {(["topics", "notes"] as const).map((value) => (
              <button
                key={value}
                type="button"
                className="luna-pill-toggle"
                aria-pressed={mode === value}
                onClick={() => setMode(value)}
              >
                {t(value === "topics" ? "graph.modeTopics" : "graph.modeNotes")}
              </button>
            ))}
          </div>
        </section>

        <section className="luna-graph-side__section" aria-label={t("graph.filters")}>
          <h3 className="luna-graph-side__section-title">{t("graph.filters")}</h3>
          <div className="luna-graph-side__toggles" role="group" aria-label={t("graph.filters")}>
            {mode === "notes" && (
              <>
                <button
                  type="button"
                  className="luna-pill-toggle"
                  aria-pressed={showTags}
                  onClick={toggleTags}
                >
                  <span
                    className="luna-graph-side__node-dot luna-graph-side__node-dot--tag"
                    aria-hidden="true"
                  />
                  {t("graph.showTags")}
                </button>
                <button
                  type="button"
                  className="luna-pill-toggle"
                  aria-pressed={showGhosts}
                  onClick={toggleGhosts}
                >
                  <span
                    className="luna-graph-side__node-dot luna-graph-side__node-dot--ghost"
                    aria-hidden="true"
                  />
                  {t("graph.showGhosts")}
                </button>
              </>
            )}
            <button
              type="button"
              className="luna-pill-toggle"
              aria-pressed={showOrphans}
              onClick={toggleOrphans}
            >
              {t("graph.showOrphans")}
            </button>
          </div>
        </section>

        {/* 笔记类型（ADR-024）：资料区默认不显示，一键可见——这是视图态，构图与索引不变。 */}
        {mode === "notes" && (
          <section className="luna-graph-side__section" aria-label={t("graph.noteTypes")}>
            <h3 className="luna-graph-side__section-title">{t("graph.noteTypes")}</h3>
            {NOTE_TYPE_GROUPS.map((group) => (
              <div key={group.id} className="luna-graph-side__type-group">
                <p className="luna-graph-side__type-group-title">{t(`graph.${group.label}`)}</p>
                <div
                  className="luna-graph-side__toggles"
                  role="group"
                  aria-label={t(`graph.${group.label}`)}
                >
                  {NOTE_TYPE_FILTERS.filter((entry) => entry.group === group.id).map((entry) => (
                    <button
                      key={entry.type}
                      type="button"
                      className="luna-pill-toggle"
                      aria-pressed={noteTypes.includes(entry.type)}
                      onClick={() => toggleNoteType(entry.type)}
                    >
                      <span
                        className={`luna-graph-side__node-dot luna-graph-side__node-dot--${
                          entry.type === "quick-note" ? "quick" : entry.type
                        }`}
                        aria-hidden="true"
                      />
                      {t(entry.label)}
                    </button>
                  ))}
                </div>
                {/* 成熟度轴（ADR-086）：另起一行、带自己的小标题、**无类型色点**——三处线索
                    一起说同一件事「这不是第六种类型」。记录节点在图上仍按它自己的类型着色，
                    所以这里没有色点可给；而那正是最强的一条线索。 */}
                {group.id === MATURITY_FILTER_GROUP && (
                  <div className="luna-graph-side__maturity">
                    <p className="luna-graph-side__type-group-title">{t("graph.maturity")}</p>
                    <div
                      className="luna-graph-side__toggles"
                      role="group"
                      aria-label={t("graph.maturity")}
                    >
                      <button
                        type="button"
                        className="luna-pill-toggle"
                        aria-pressed={showDrafts}
                        title={t("graph.showDraftsHint")}
                        onClick={toggleDrafts}
                      >
                        {t("graph.showDrafts")}
                      </button>
                    </div>
                  </div>
                )}
              </div>
            ))}
          </section>
        )}

        {focus && (
          <section className="luna-graph-side__section" aria-label={t("graph.focusLabel")}>
            <h3 className="luna-graph-side__section-title">{t("graph.focusLabel")}</h3>
            <div className="luna-graph__focus-chip">
              <span className="luna-graph__focus-name" title={focusLabel ?? undefined}>
                {focusLabel}
              </span>
              {([1, 2] as const).map((depth) => (
                <button
                  key={depth}
                  type="button"
                  className="luna-pill-toggle"
                  aria-pressed={focus.depth === depth}
                  onClick={() => setFocus({ id: focus.id, depth })}
                >
                  {t("graph.depth", { depth })}
                </button>
              ))}
              <button
                type="button"
                className="luna-pill-toggle"
                aria-label={t("graph.exitFocus")}
                title={t("graph.exitFocus")}
                onClick={() => setFocus(null)}
              >
                <CloseIcon size={12} />
              </button>
            </div>
          </section>
        )}

        <section className="luna-graph-side__section" aria-label={t("graph.nextAction.title")}>
          <h3 className="luna-graph-side__section-title">{t("graph.nextAction.title")}</h3>
          {activeMaintenance ? (
            /* 运行中占用推荐槽：进度**与取消**同处一位，下面绝不再留一个「再开一次」。 */
            <div className="luna-next-action" role="status" aria-live="polite">
              <strong>{t("graph.nextAction.runningTitle")}</strong>
              <p className="luna-next-action__desc">{activeMaintenance}</p>
              {rescanProgress ? (
                <>
                  <p className="luna-next-action__desc">
                    {rescanProgress.scope === null
                      ? t("graph.rescanRunning", {
                          done: rescanProgress.done,
                          total: rescanProgress.total,
                        })
                      : t("graph.rescanRunningScoped", {
                          folder: rescanProgress.scope,
                          done: rescanProgress.done,
                          total: rescanProgress.total,
                        })}
                  </p>
                  <progress
                    className="luna-progress luna-graph-side__progress-bar"
                    max={rescanProgress.total}
                    value={rescanProgress.done}
                  />
                  {rescanProgress.failed > 0 && (
                    <p className="luna-field-error">
                      {t("graph.rescanFailed", { count: rescanProgress.failed })}
                    </p>
                  )}
                </>
              ) : null}
              {rescanning ? (
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary"
                  title={t("graph.rescanCancelHint")}
                  onClick={cancelRescan}
                >
                  {t("graph.rescanCancel")}
                </button>
              ) : null}
            </div>
          ) : (
            /* 推荐卡与「知识库 → 概览」**共用同一个组件**：三个建议表面此前形态各异
               （横幅 / 空态句子 / 维护卡），用户第一步要做的判断成了「这三个是不是同一件
               事」。共用让「同一份状态、同一套动作语义」成为结构保证（PRD-16-016）。
               运行中那一档留在本文件——重扫进度与取消是图谱自己的状态。 */
            <KnowledgeNextActionCard
              scope="graph"
              reloadKey={nextActionReload}
              onReloaded={() => setNextActionReload((n) => n + 1)}
              onAction={setRecommended}
            />
          )}
          <button
            type="button"
            className="luna-pill-toggle"
            ref={maintenanceToggleRef}
            aria-expanded={showMaintenance}
            aria-controls="knowledge-maintenance-details"
            onClick={() => setShowMaintenance((shown) => !shown)}
          >
            {t(
              showMaintenance
                ? "graph.nextAction.hideMaintenance"
                : "graph.nextAction.moreMaintenance",
            )}
          </button>
        </section>

        {showMaintenance ? (
          <section
            id="knowledge-maintenance-details"
            className="luna-graph-side__section"
            aria-label={t("graph.nextAction.maintenanceTitle")}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                setShowMaintenance(false);
                maintenanceToggleRef.current?.focus();
              }
            }}
          >
            {/* 折叠区的标题与按钮一律用**用户目的**文案（PRD-16-039）：
              「AI 关系标签 / 全库重扫 / 批量整理」是实现词，只进高级诊断。 */}
            <h3 className="luna-graph-side__section-title">
              {t("graph.nextAction.maintenanceTitle")}
            </h3>
            <p className="luna-graph-side__desc">{t("graph.nextAction.maintenanceDesc")}</p>
            {/* 运行中的进度（含数字读出与取消）**只在推荐位出现一次**：此前这里还画
                一条同源进度条，同一件事同屏两条，两个 `aria-live` 区会让读屏念两遍。
                `rescanning ⇒ activeMaintenance`，故运行中整个折叠区只保留其余维护入口。
                推荐卡此刻的 CTA 就是重扫时（`recommended === "rescan_tags"`），这颗
                手动入口同样让位：同一个动作同屏两枚，用户第一件事就是猜它们是不是同一
                件事（真机第四轮）。语言收敛档（`rescan_tags_force`）不算重复——「全部
                重新生成」与「只补缺口」是两件代价差很远的事。 */}
            {rescanProgress || recommended === "rescan_tags" ? null : (
              <button
                type="button"
                className="luna-btn luna-btn--secondary luna-graph-side__cta"
                onClick={() => openRescanDialog(null)}
              >
                <TagsIcon size={14} />
                <span>{t("graph.nextAction.maintenanceConnect")}</span>
              </button>
            )}
            {/* 「整理现有资料」已移出本区、落「知识库 → 概览」（PRD-16-039 2026-08-30
                修订）：它产出的是笔记，用户想做它时会去看自己有多少资料——那个数字在
                概览的现状行里，不在图谱。留在这里的两颗各有理由：重扫由 PRD-06-035b
                明文要求入口在图谱（动机与结果同屏），体检的孤儿页与断链本就是图谱概念。 */}
            {/* 知识体检（PRD-16-030）：孤儿页与断链本就是**图谱概念**（孤立点、幽灵节点），
              维护动机也正是在这儿产生的（"这张图怎么散成这样"）。报告面板由 Shell 常驻宿主
              承载，与重扫/批量整理同一模式。 */}
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-graph-side__cta"
              onClick={() => requestKnowledgeCheckup()}
            >
              <StethoscopeIcon size={14} />
              <span>{t("graph.nextAction.maintenanceCheck")}</span>
              {/* 待收拾的处数（与启动概览那一行同一口径：积压不计入——它在别处已经在说了）。
                有数字才渲染：这个按钮本身常驻，徽标只在真有东西时说话。 */}
              {hygiene > 0 ? (
                <span className="luna-count-badge luna-graph-side__cta-badge">{hygiene}</span>
              ) : null}
            </button>
          </section>
        ) : null}
      </div>
    </section>
  );
}
