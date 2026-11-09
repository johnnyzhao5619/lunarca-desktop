/**
 * 创作空间侧栏（ADR-162 / PRD-19-002 / PRD-19-005）：两份清单——视频项目与灵感条目。
 *
 * 项目清单与主区看板同一份数据（`creator_board`，按主页修改时间倒序），不另查一遍；
 * 灵感清单是 `type: swipe` 的分页查询，与录音 / 转写清单同一条通路（`search` + 游标）。
 * 点项目进它的概览页（ADR-162 ⑫），点灵感打开那篇笔记；新建由 Rust 决定落哪个目录、叫什么名字
 * （ADR-003）。
 *
 * 项目清单可按阶段筛选（只是这一处的显示，不改任何东西）；已归档的项目不在清单里，单独收在
 * 「已归档」一节（默认收起，ADR-165 S6），归档与否由 Rust 给（`board.archived`）。
 *
 * 每一行右键或悬停出现的「···」打开它的菜单（ADR-167 ①）：项目是与看板、页头同一份的项目菜单；
 * 灵感是笔记菜单，由宿主组装（`onSwipeMenu`）。
 */

import { useState, type MouseEvent as ReactMouseEvent } from "react";
import { useTranslation } from "react-i18next";
import { commands, type ProjectCardDto, type ProjectStageDto, type SearchHitDto } from "../../ipc/bindings";
import { CaptureStartButton } from "../../components/ui/CaptureStartButton";
import { EmptyState } from "../../components/ui/EmptyState";
import { IconButton } from "../../components/ui/IconButton";
import { BookOpenIcon, BookmarkIcon, ChevronRightIcon, EllipsisIcon, FilmIcon, PlusIcon } from "@lunarca/editor-core/Icon";
import { SelectBox } from "../../components/ui/SelectBox";
import { formatDateTime } from "../../lib/format-datetime";
import { ListLoadMore } from "../workspace/ListLoadMore";
import { pageOf, usePagedList } from "../workspace/paged-list";
import { stageLabelKey } from "./creator-board";
import { useProjectMenu, type ProjectMenuNav } from "./project-menu";
import type { CreatorBoardState } from "./use-creator-board";

/** 阶段筛选的「全部」。 */
const ALL_STAGES = "all";

export interface CreatorSidebarProps {
  board: CreatorBoardState;
  /** 库版本（`vaultRevision`）：变了就重读灵感清单。 */
  revision: number;
  enabled: boolean;
  activePath: string | null;
  onOpen: (path: string) => void;
  /** 点项目：进它的概览页。 */
  onOpenProject: (path: string, title: string) => void;
  onCreateProject: () => void;
  onCreateSwipe: () => void;
  /** 打开灵感库页（按套路分组、归类套路，ADR-162 ⑬）。 */
  onOpenLibrary: () => void;
  /** 项目行的菜单：导航与刷新（与看板、页头同一份项目菜单）。 */
  projectNav: ProjectMenuNav;
  /** 灵感行的菜单（笔记菜单，宿主组装）。 */
  onSwipeMenu: (event: ReactMouseEvent, path: string) => void;
}

export function CreatorSidebar({
  board,
  revision,
  enabled,
  activePath,
  onOpen,
  onOpenProject,
  onCreateProject,
  onCreateSwipe,
  onOpenLibrary,
  projectNav,
  onSwipeMenu,
}: CreatorSidebarProps) {
  const { t, i18n } = useTranslation("workspace");
  const swipes = usePagedList<SearchHitDto>({
    key: `swipes|${revision}`,
    enabled,
    fetchPage: async (cursor) => {
      const result = await commands.search({
        text: null,
        note_type: "swipe",
        tag: null,
        path_prefix: null,
        name_like: null,
        limit: null,
        page: { sort: "modified", cursor },
      });
      return result.status === "ok" ? pageOf(result.data) : null;
    },
  });
  const [stageFilter, setStageFilter] = useState<ProjectStageDto | typeof ALL_STAGES>(ALL_STAGES);
  const [showArchived, setShowArchived] = useState(false);
  const allProjects = board.board?.projects ?? [];
  const archived = board.board?.archived ?? [];
  const stages = board.board ? [...board.board.stages, ...board.board.hidden_stages] : [];
  const projects =
    stageFilter === ALL_STAGES ? allProjects : allProjects.filter((project) => project.stage === stageFilter);
  const openProjectMenu = useProjectMenu(projectNav, stages);

  const row = (
    path: string,
    title: string,
    modifiedMs: number | null,
    open: () => void,
    openMenu: (event: ReactMouseEvent) => void,
  ) => (
    <li key={path} role="none" className="luna-creator-list__row" onContextMenu={openMenu}>
      <button
        type="button"
        role="option"
        aria-selected={path === activePath}
        className="luna-creator-list__item"
        title={path}
        onClick={open}
      >
        <span className="luna-creator-list__title">{title}</span>
        <span className="luna-creator-list__time">{formatDateTime(modifiedMs, i18n.language)}</span>
      </button>
      <IconButton
        className="luna-creator-list__more"
        density="sm"
        label={t("creator.sidebar.more", { name: title })}
        icon={<EllipsisIcon size={14} />}
        aria-haspopup="menu"
        onClick={openMenu}
      />
    </li>
  );
  const projectRow = (project: ProjectCardDto, isArchived: boolean) =>
    row(
      project.path,
      project.title,
      project.modified_ms,
      () => onOpenProject(project.path, project.title),
      (event) =>
        openProjectMenu(event, { home: project.path, title: project.title, stage: project.stage, archived: isArchived }),
    );

  return (
    <div className="luna-creator-sidebar">
      <CaptureStartButton
        icon={<PlusIcon size={14} />}
        label={t("creator.newProject")}
        onClick={onCreateProject}
      />

      <section className="luna-creator-sidebar__section" aria-labelledby="luna-creator-projects">
        <header className="luna-sidebar-header">
          <div className="luna-sidebar-header__title">
            <h2 id="luna-creator-projects">{t("creator.projects")}</h2>
            {board.board ? <span className="luna-count-badge">{allProjects.length}</span> : null}
          </div>
        </header>
        {allProjects.length > 0 ? (
          <SelectBox
            size="sm"
            value={stageFilter}
            ariaLabel={t("creator.sidebar.filterLabel")}
            onChange={(event) => setStageFilter(event.currentTarget.value as ProjectStageDto | typeof ALL_STAGES)}
          >
            <option value={ALL_STAGES}>{t("creator.sidebar.filterAll")}</option>
            {stages.map((stage) => (
              <option key={stage} value={stage}>
                {t(stageLabelKey(stage))}
              </option>
            ))}
          </SelectBox>
        ) : null}
        {board.board && allProjects.length === 0 ? (
          <EmptyState
            icon={<FilmIcon size={24} />}
            title={t("creator.emptyTitle")}
            description={t("creator.emptyDescription")}
          />
        ) : projects.length === 0 && board.board ? (
          <p className="luna-creator-sidebar__hint">{t("creator.sidebar.filterEmpty")}</p>
        ) : (
          <ul className="luna-creator-list" role="listbox" aria-label={t("creator.projects")}>
            {projects.map((project) => projectRow(project, false))}
          </ul>
        )}
        {archived.length > 0 ? (
          <div className="luna-creator-sidebar__archived">
            <button
              type="button"
              className="luna-creator-sidebar__toggle"
              aria-expanded={showArchived}
              onClick={() => setShowArchived((open) => !open)}
            >
              <ChevronRightIcon size={12} />
              {t("creator.sidebar.archived")}
              <span className="luna-count-badge">{archived.length}</span>
            </button>
            {showArchived ? (
              <ul className="luna-creator-list" role="listbox" aria-label={t("creator.sidebar.archived")}>
                {archived.map((project) => projectRow(project, true))}
              </ul>
            ) : null}
          </div>
        ) : null}
      </section>

      <section className="luna-creator-sidebar__section" aria-labelledby="luna-creator-swipes">
        <header className="luna-sidebar-header">
          <div className="luna-sidebar-header__title">
            <h2 id="luna-creator-swipes">{t("creator.swipes")}</h2>
            {swipes.exhausted ? <span className="luna-count-badge">{swipes.items.length}</span> : null}
            <IconButton
              label={t("creator.swipeLibrary.open")}
              icon={<BookOpenIcon size={14} />}
              onClick={onOpenLibrary}
            />
            <IconButton
              label={t("creator.newSwipe")}
              icon={<PlusIcon size={14} />}
              onClick={onCreateSwipe}
            />
          </div>
        </header>
        {swipes.exhausted && !swipes.loading && !swipes.failed && swipes.items.length === 0 ? (
          <EmptyState
            icon={<BookmarkIcon size={24} />}
            title={t("creator.swipesEmptyTitle")}
            description={t("creator.swipesEmptyDescription")}
          />
        ) : (
          <ul className="luna-creator-list" role="listbox" aria-label={t("creator.swipes")}>
            {swipes.items.map((hit) =>
              row(hit.path, hit.title, hit.modified_ms, () => onOpen(hit.path), (event) => onSwipeMenu(event, hit.path)),
            )}
          </ul>
        )}
        <ListLoadMore
          loading={swipes.loading}
          exhausted={swipes.exhausted}
          failed={swipes.failed}
          onLoadMore={swipes.loadMore}
          onRetry={() => void swipes.reload()}
        />
      </section>
    </div>
  );
}
