/**
 * 创作空间主区：视频项目的阶段看板（ADR-162 / PRD-19-003）。
 *
 * 一列一个阶段（列序由 Rust 给）；拖动卡片到另一列 = 改写项目主页的 `luna.project_stage`。
 * 拖拽走全应用共用的指针拖拽基元（`lib/pointer-drag.ts`），不用 HTML5 DnD。键盘与不便
 * 拖拽时，卡片右键「移到 ▸」做同一件事。点卡片打开项目主页。
 *
 * 列可按需显隐（PRD-19-003，至少一列）：页头「看板列」菜单逐列勾选，列头右键「隐藏这一列」，
 * 排序在设置页。停在隐藏列上的项目借住在它前面最近的显示列里，并标出真实阶段（Rust 判定）。
 */

import { useEffect, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
import { useTranslation } from "react-i18next";
import type { CreatorBoardViewDto, ProjectCardDto, ProjectStageDto } from "../../ipc/bindings";
import { useContextMenu } from "../../components/ContextMenuHost";
import { EmptyState } from "../../components/ui/EmptyState";
import { FilmIcon } from "../../components/ui/Icon";
import { buildMenu } from "../../lib/context-menu";
import { cssVariables } from "../../lib/css-variables";
import { DRAG_GHOST_OFFSET_PX, usePointerDrag } from "../../lib/pointer-drag";
import { columnsOf, stageLabelKey, stageOfDropId } from "./creator-board";
import { CreatorCalendar } from "./CreatorCalendar";
import { CreatorPageHeader, PageLoading, PageNotice, usePageNotice } from "./CreatorPage";
import { useProjectMenu, type ProjectMenuNav } from "./project-menu";
import { BoardColumnsView, ListView, TableView } from "./CreatorViews";
import type { CreatorBoardState } from "./use-creator-board";

/** 视图切换控件的顺序（与设置 `creator.board_view` 的选项同序）。 */
const VIEWS: readonly CreatorBoardViewDto[] = ["board", "list", "table", "calendar"];

export interface CreatorBoardProps {
  state: CreatorBoardState;
  onCreateProject: () => void;
  /** 打开设置里的「创作看板的列」（排序在那里）。 */
  onOpenColumnSettings: () => void;
  /** 打开这个项目（项目首页，ADR-165 ⑥）：点卡片的默认动作。 */
  onOpenProject: (card: ProjectCardDto) => void;
  /** 卡片右键菜单的导航与刷新（与侧栏、项目页头同一份菜单，ADR-167 ①）。 */
  projectNav: ProjectMenuNav;
}

export function CreatorBoard({
  state,
  onCreateProject,
  onOpenColumnSettings,
  onOpenProject,
  projectNav,
}: CreatorBoardProps) {
  const { t } = useTranslation("workspace");
  const menu = useContextMenu();
  // 回到看板就重读一次：在项目里改的渠道、计划日期、发布进度只写了项目主页（`note://written`），
  // 不触发库变动的重读，看板上的卡片信号会停在进项目之前（S1 真机发现）。
  const { reload } = state;
  useEffect(() => {
    reload();
  }, [reload]);
  // 改阶段、改日期、切视图失败：就近写在页头下面（ADR-165 ⑨），不进状态栏。
  const { notice, fail, clear } = usePageNotice();
  /** 正在写盘的那一张（拖完到 Rust 回答之前不让它再被拖）。 */
  const [moving, setMoving] = useState<string | null>(null);
  const stages = state.board?.stages ?? [];
  const hiddenStages = state.board?.hidden_stages ?? [];
  /** 只剩一列时它收不起来（至少一列，Rust 归一化同样兜底）。 */
  const lastColumn = stages.length <= 1;

  const switchView = async (view: CreatorBoardViewDto) => {
    if (state.board?.view === view) {
      return;
    }
    const error = await state.setView(view);
    if (error) {
      fail(error);
    }
  };

  const setVisible = async (stage: ProjectStageDto, visible: boolean) => {
    const error = await state.setStageVisible(stage, visible);
    if (error) {
      fail(error);
    }
  };

  /** 「看板列」菜单：显示的列打勾、收起的不打；最后一列的勾去不掉；排序去设置页。 */
  const openColumnsMenu = (event: ReactMouseEvent) => {
    menu.open(
      event,
      buildMenu([
        ...[...stages, ...hiddenStages].map((stage) => {
          const visible = stages.includes(stage);
          return {
            kind: "action" as const,
            id: `creator_column_${stage}`,
            label: t(stageLabelKey(stage)),
            checked: visible,
            disabled: visible && lastColumn,
            onSelect: () => void setVisible(stage, !visible),
          };
        }),
        { kind: "separator", id: "creator_columns_sep" },
        {
          kind: "action",
          id: "creator_columns_settings",
          label: t("creator.columnsInSettings"),
          onSelect: onOpenColumnSettings,
        },
      ]),
    );
  };

  const openColumnMenu = (event: ReactMouseEvent, stage: ProjectStageDto) => {
    menu.open(
      event,
      buildMenu([
        {
          kind: "action",
          id: "creator_column_hide",
          label: t("creator.hideColumn", { stage: t(stageLabelKey(stage)) }),
          disabled: lastColumn,
          onSelect: () => void setVisible(stage, false),
        },
        hiddenStages.length > 0 && {
          kind: "submenu",
          id: "creator_column_show",
          label: t("creator.showHiddenColumns"),
          items: hiddenStages.map((hidden) => ({
            kind: "action" as const,
            id: `creator_column_show_${hidden}`,
            label: t(stageLabelKey(hidden)),
            onSelect: () => void setVisible(hidden, true),
          })),
        },
        { kind: "separator", id: "creator_column_sep" },
        {
          kind: "action",
          id: "creator_column_settings",
          label: t("creator.columnsInSettings"),
          onSelect: onOpenColumnSettings,
        },
      ]),
    );
  };

  const move = async (card: ProjectCardDto, stage: ProjectStageDto) => {
    if (card.stage === stage) {
      return;
    }
    setMoving(card.path);
    const error = await state.moveProject(card.path, stage);
    setMoving(null);
    if (error) {
      fail(error);
    }
  };

  const setDate = async (card: ProjectCardDto, date: string | null) => {
    setMoving(card.path);
    const error = await state.setPublishDate(card.path, date);
    setMoving(null);
    if (error) {
      fail(error);
    }
  };

  /**
   * 刚拖完的那一张：松手若仍落在卡片上，浏览器紧接着会派发一次 click——那不是「打开」的意图。
   * 下一个任务里清掉，所以只吞掉紧跟着的那一次。
   */
  const justDragged = useRef<string | null>(null);
  const endDrag = (card: ProjectCardDto) => {
    justDragged.current = card.path;
    window.setTimeout(() => {
      justDragged.current = null;
    }, 0);
  };
  const drag = usePointerDrag<ProjectCardDto>(
    (card, dropId) => {
      endDrag(card);
      const stage = stageOfDropId(dropId, stages);
      // 放回它此刻所在的那一列是取消，不是「改成这一列的阶段」——借住的卡片也一样。
      if (stage && stage !== card.column) {
        void move(card, stage);
      }
    },
    (card) => endDrag(card),
  );
  const hoverStage = drag.dropId === null ? null : stageOfDropId(drag.dropId, stages);

  const openProjectMenu = useProjectMenu(projectNav, [...stages, ...hiddenStages]);
  const openCardMenu = (event: ReactMouseEvent, card: ProjectCardDto) =>
    openProjectMenu(event, { home: card.path, title: card.title, stage: card.stage, archived: false });

  // 页头（ADR-165 ⑨）：没有主动作——新建项目在侧栏与空态里，页头不再放第二个。
  const header = (
    <>
      <CreatorPageHeader
        title={t("nav.creator")}
        secondary={
          state.board ? (
            <>
              {/* 视图切换：选择记在设置里（`creator.board_view`），下次进来还是它。 */}
              <div className="luna-segmented-control" role="radiogroup" aria-label={t("creator.view.label")}>
                {VIEWS.map((view) => (
                  <button
                    key={view}
                    type="button"
                    role="radio"
                    aria-checked={state.board?.view === view}
                    className="luna-segmented-control__item"
                    data-active={state.board?.view === view}
                    onClick={() => void switchView(view)}
                  >
                    {t(`creator.view.${view}`)}
                  </button>
                ))}
              </div>
              {state.board.view === "calendar" ? null : (
                <button type="button" className="luna-btn luna-btn--secondary" onClick={openColumnsMenu}>
                  {t("creator.columns")}
                </button>
              )}
            </>
          ) : null
        }
      />
      <PageNotice notice={notice} onDismiss={clear} />
    </>
  );

  if (state.board === null) {
    return (
      <section className="luna-creator">
        {header}
        {state.error ? null : <PageLoading />}
        {state.error ? (
          <EmptyState
            icon={<FilmIcon size={24} />}
            title={t("creator.loadFailedTitle")}
            description={state.error}
            action={
              <button type="button" className="luna-btn luna-btn--secondary" onClick={state.reload}>
                {t("creator.retry")}
              </button>
            }
          />
        ) : null}
      </section>
    );
  }

  if (state.board.projects.length === 0) {
    return (
      <section className="luna-creator">
        {header}
        <EmptyState
          icon={<FilmIcon size={24} />}
          title={t("creator.emptyTitle")}
          description={t("creator.emptyDescription")}
          action={
            <button type="button" className="luna-btn luna-btn--primary" onClick={onCreateProject}>
              {t("creator.newProject")}
            </button>
          }
        />
      </section>
    );
  }

  const board = state.board;
  const viewProps = {
    columns: columnsOf(stages, board.projects),
    stages,
    drag,
    hoverStage,
    moving,
    onCardClick: (card: ProjectCardDto) => {
      if (justDragged.current !== card.path) {
        onOpenProject(card);
      }
    },
    onCardMenu: openCardMenu,
    onColumnMenu: openColumnMenu,
    onMove: (card: ProjectCardDto, stage: ProjectStageDto) => void move(card, stage),
  };

  return (
    <section className="luna-creator" aria-busy={state.loading || moving !== null}>
      {header}
      {board.view === "calendar" ? (
        <CreatorCalendar
          projects={board.projects}
          moving={moving}
          onOpen={onOpenProject}
          onCardMenu={openCardMenu}
          onSetDate={(card, date) => void setDate(card, date)}
        />
      ) : board.view === "list" ? (
        <ListView {...viewProps} />
      ) : board.view === "table" ? (
        <TableView {...viewProps} />
      ) : (
        <BoardColumnsView {...viewProps} />
      )}
      {drag.payload ? (
        <div
          className="luna-drag-ghost"
          style={cssVariables({
            "--drag-x": `${drag.position.x + DRAG_GHOST_OFFSET_PX}px`,
            "--drag-y": `${drag.position.y + DRAG_GHOST_OFFSET_PX}px`,
          })}
        >
          {drag.payload.title}
        </div>
      ) : null}
    </section>
  );
}
