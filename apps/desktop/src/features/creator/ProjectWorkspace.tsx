/**
 * 项目工作区（ADR-165 ⑥ / PRD-19-037）：打开一个视频项目，就停在这里把它做完。
 *
 * - 页头：面包屑（创作 › 项目名）、阶段步进条（点选即改阶段，写回主页 frontmatter）、一个随阶段
 *   变化的主动作（映射在 Rust：`luna_vault::next_action`）；步进条右端是计划发布日期（PRD-19-040，
 *   写回 `luna.project_publish_date`，日历视图读同一个字段）。
 * - 页签：概览（= 项目首页：主页笔记就地编辑 + 「项目里的东西」）· 研究 · 脚本 · 录制 · 发布 · 分发；从项目里
 *   点开的其它笔记作为一个临时页签出现，可关。
 * - 编辑器与「笔记」空间是同一个组件（宿主经 `renderEditor` 给出），项目里的笔记不再被带去
 *   「笔记」空间；「在笔记中打开」是次要动作。
 *
 * 哪篇是研究、哪篇是脚本、阶段与下一步都由 Rust 一次给齐（`creator_project`），这里只渲染。
 */

import { useEffect, useRef, useState, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

import {
  commands,
  type NextActionDto,
  type ProjectStageDto,
  type ProjectWorkspaceDto,
  type ScriptOutlineDto,
} from "../../ipc/bindings";
import { EmptyState } from "../../components/ui/EmptyState";
import { IconButton } from "../../components/ui/IconButton";
import { CloseIcon, EllipsisIcon, FilmIcon } from "@lunarca/editor-core/Icon";
import { useTablistNavigation } from "../../hooks/use-tablist-navigation";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { requestEditorReveal } from "../../lib/app-events";
import type { ContextMenuEntry } from "../../lib/context-menu";
import { vaultErrorText } from "../../lib/vault-error";
import type { CreateActionContext } from "../capture/create-actions";
import type { CreatorTab } from "../workspace/workspace-routes";
import { stageLabelKey } from "./creator-board";
import { CreatorPageHeader, PageNotice, usePageNotice } from "./CreatorPage";
import { DistributeView } from "./DistributeView";
import { ProjectOverview } from "./ProjectOverview";
import { RecordingPanel } from "./RecordingPanel";
import { ScriptInspector } from "./ScriptInspector";
import { ScriptToolbar } from "./ScriptToolbar";
import { AddSourcesDialog } from "./AddSourcesDialog";
import { subscribeProjectChanges, useProjectMenu, type ProjectMenuNav } from "./project-menu";
import { useMaterialImport } from "./use-material-import";

/** 项目工作区里此刻站的页签：四个固定页签，或一篇从项目里点开的笔记。 */
export type ProjectPane = { tab: CreatorTab } | { tab: "doc"; path: string };

export interface ProjectWorkspaceProps {
  home: string;
  pane: ProjectPane;
  onPane: (pane: ProjectPane) => void;
  /** 面包屑「创作」：回看板。 */
  onBoard: () => void;
  /**
   * 在主区放一篇笔记的编辑器（与「笔记」空间同一个组件，度量与上下文面板由宿主接）。
   * `openPath`：编辑器里点到的链接在项目里怎么开（主页 / 研究 / 脚本各回自己的页签）。
   */
  renderEditor: (path: string, openPath: (path: string) => void) => ReactNode;
  /** 发布页签的内容（发布包页，嵌在页签里、不带自己的页头）。 */
  renderPublish: (project: { home: string; title: string }) => ReactNode;
  /** 素材已排进处理队列（打开队列看进度）。 */
  onImportQueued: () => void;
  /** 次要动作「在笔记中打开」。 */
  onOpenInNotes: (path: string) => void;
  /** 此刻主区里在编辑的那一篇（上下文面板、状态栏据此跟上）；没有编辑器时为 `null`。 */
  onActiveDocument: (path: string | null) => void;
  /** 编辑器给出的脚本分段与时长（只有脚本页签用；宿主按路径对齐后转交，不是这一篇为 `null`）。 */
  scriptOutline: ScriptOutlineDto | null;
  /** 录制页签的开录前检查要用的修复入口（与「添加内容」同一份）。 */
  createActions: CreateActionContext;
  /** 页头「更多」= 项目菜单（与侧栏、看板同一份，ADR-167 ①）的导航与刷新。 */
  projectNav: ProjectMenuNav;
  /** 「项目里的东西」一行的笔记菜单（宿主组装）。 */
  onItemMenu: (event: ReactMouseEvent, path: string, leading?: ContextMenuEntry[]) => void;
}

function stemOf(path: string): string {
  const name = path.split("/").pop() ?? path;
  return name.endsWith(".md") ? name.slice(0, -3) : name;
}

export function ProjectWorkspace({
  home,
  pane,
  onPane,
  onBoard,
  renderEditor,
  renderPublish,
  onImportQueued,
  onOpenInNotes,
  onActiveDocument,
  scriptOutline,
  createActions,
  projectNav,
  onItemMenu,
}: ProjectWorkspaceProps) {
  const { t } = useTranslation("workspace");
  const [project, setProject] = useState<ProjectWorkspaceDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [savingStage, setSavingStage] = useState(false);
  /** 没排期时先是一个「设计划日期」按钮：WebKit 的空日期框会把今天淡淡地画出来，像是已经排了。 */
  const [pickingDate, setPickingDate] = useState(false);
  /** 收起了哪一条阶段推进提示（`当前阶段>建议阶段`；只管这一次打开的这个项目）。 */
  const [dismissedHint, setDismissedHint] = useState<string | null>(null);
  const requestRef = useRef(0);
  const materials = useMaterialImport(home, onImportQueued);
  // 即时操作的结果就近说（ADR-165 ⑨）：改阶段、改日期、开提词器失败都写在页头下面。
  const { notice, fail, clear } = usePageNotice();

  const load = () => {
    const request = ++requestRef.current;
    void commands.creatorProject(home).then((reply) => {
      if (request !== requestRef.current) return;
      if (reply.status !== "ok") {
        setError(vaultErrorText(reply.error));
        return;
      }
      setError(null);
      setProject(reply.data);
    });
  };
  useEffect(load, [home]);
  // 改名、删掉脚本、外部改了阶段：重取一次。
  useVaultReload(load);
  // 侧栏或看板的项目菜单改了这个项目（阶段、归档）：页头跟着重读。
  useEffect(() => subscribeProjectChanges((changed) => changed === home && load()), [home]);

  const tabs: CreatorTab[] = ["overview"];
  if (project?.research) tabs.push("research");
  // 录制靠脚本跟读：没有脚本就没有录制页签。
  if (project?.script) tabs.push("script", "recording");
  tabs.push("publish", "distribute");
  // 页签不在了（脚本被删）就落回概览，而不是停在一个空页签上。
  const tab: CreatorTab | "doc" = pane.tab === "doc" || tabs.includes(pane.tab) ? pane.tab : "overview";

  /** 这一刻编辑器里放的是哪一篇（概览 = 主页；发布页签没有编辑器）。 */
  const editorPath: string | null =
    pane.tab === "doc"
      ? pane.path
      : tab === "overview"
        ? home
        : tab === "research"
          ? (project?.research ?? null)
          : tab === "script"
            ? (project?.script ?? null)
            : null;

  useEffect(() => {
    onActiveDocument(editorPath);
  }, [editorPath, onActiveDocument]);
  useEffect(() => () => onActiveDocument(null), [onActiveDocument]);

  const navigation = useTablistNavigation<CreatorTab>(tabs, tab === "doc" ? "overview" : tab, (next) =>
    onPane({ tab: next }),
  );

  const setStage = async (stage: ProjectStageDto) => {
    if (savingStage || project?.stage === stage) return;
    setSavingStage(true);
    const reply = await commands.creatorSetProjectStage(home, stage);
    setSavingStage(false);
    if (reply.status === "error") {
      fail(vaultErrorText(reply.error));
      return;
    }
    load();
  };

  const setPublishDate = async (date: string | null) => {
    if ((project?.publish_date ?? null) === date) return;
    const reply = await commands.creatorSetPublishDate(home, date);
    if (reply.status === "error") {
      fail(vaultErrorText(reply.error));
      return;
    }
    load();
  };

  const runNext = async (action: NextActionDto) => {
    switch (action) {
      case "write_idea":
        onPane({ tab: "overview" });
        return;
      case "import_material":
        // 研究阶段的下一步是「添加来源」：在哪个页签都打开同一个对话框（ADR-167 ③）。
        materials.openAdd();
        return;
      case "open_teleprompter": {
        if (!project?.script) return;
        const reply = await commands.teleprompterOpen(project.script, false);
        if (reply.status === "error") fail(vaultErrorText(reply.error));
        return;
      }
      case "start_follow":
        // 先到录制页签看一眼开录前检查，再由那里的主按钮开跟读。
        onPane({ tab: project?.script ? "recording" : "overview" });
        return;
      case "import_final_cut":
      case "open_publish":
        onPane({ tab: "publish" });
        return;
      case "distribute":
        onPane({ tab: "distribute" });
        return;
    }
  };

  const title = project?.title ?? stemOf(home);

  const openProjectMenu = useProjectMenu(projectNav, project?.stages ?? []);
  const openMore = (event: ReactMouseEvent) => {
    if (!project) return;
    openProjectMenu(event, { home, title, stage: project.stage, archived: project.archived });
  };
  const openInProject = (path: string) => onPane(paneFor(path, home, project));

  if (error !== null && project === null) {
    return (
      <section className="luna-project">
        <EmptyState
          icon={<FilmIcon size={24} />}
          title={t("creator.project.missingTitle")}
          description={error}
          action={
            <button type="button" className="luna-btn luna-btn--secondary" onClick={onBoard}>
              {t("creator.project.backToBoard")}
            </button>
          }
        />
      </section>
    );
  }

  return (
    <section className="luna-project" aria-labelledby="project-title">
      <CreatorPageHeader
        crumb={{ label: t("nav.creator"), onClick: onBoard }}
        title={title}
        titleId="project-title"
        status={project?.archived ? t("creator.project.more.archivedStatus") : undefined}
        secondary={
          <>
            {editorPath ? (
              <button type="button" className="luna-btn luna-btn--secondary" onClick={() => onOpenInNotes(editorPath)}>
                {t("creator.page.openInNotes")}
              </button>
            ) : null}
            <IconButton
              label={t("creator.project.more.label")}
              icon={<EllipsisIcon size={16} />}
              aria-haspopup="menu"
              disabled={project === null}
              onClick={openMore}
            />
          </>
        }
        primary={
          project?.next_action ? (
            <button
              type="button"
              className="luna-btn luna-btn--primary"
              onClick={() => void runNext(project.next_action as NextActionDto)}
            >
              {t(`creator.nextAction.${project.next_action}`)}
            </button>
          ) : null
        }
      />
      <PageNotice notice={notice} onDismiss={clear} />
      {materials.addOpen ? <AddSourcesDialog home={home} materials={materials} /> : null}

      {project ? (
        <div className="luna-project__status">
        <ol className="luna-project__stepper" aria-label={t("creator.project.stagesLabel")}>
          {project.stages.map((stage, index) => {
            const current = project.stages.indexOf(project.stage);
            const state = index < current ? "done" : index === current ? "current" : "todo";
            return (
              <li key={stage} className="luna-project__step" data-state={state}>
                <button
                  type="button"
                  className="luna-project__step-button"
                  aria-current={state === "current" ? "step" : undefined}
                  disabled={savingStage}
                  title={state === "current" ? undefined : t("creator.project.moveToStage", { stage: t(stageLabelKey(stage)) })}
                  onClick={() => void setStage(stage)}
                >
                  <span className="luna-project__step-dot" aria-hidden="true" />
                  {t(stageLabelKey(stage))}
                </button>
              </li>
            );
          })}
        </ol>
          <div className="luna-project__date">
            <label className="luna-project__date-label" htmlFor="project-publish-date">
              {t("creator.project.date.label")}
            </label>
            {project.publish_date || pickingDate ? (
              <input
                id="project-publish-date"
                type="date"
                className="luna-input luna-input--sm"
                value={project.publish_date ?? ""}
                autoFocus={pickingDate && !project.publish_date}
                // 日期选择器每选一次就是一个完整的值；没选完时是空串，不提交（清掉走旁边的按钮）。
                onChange={(event) => {
                  if (event.target.value) void setPublishDate(event.target.value);
                }}
                onBlur={() => setPickingDate(false)}
              />
            ) : (
              <button
                type="button"
                className="luna-btn luna-btn--secondary luna-btn--sm"
                onClick={() => setPickingDate(true)}
              >
                {t("creator.project.date.set")}
              </button>
            )}
            {project.publish_date ? (
              <IconButton
                density="sm"
                icon={<CloseIcon size={12} />}
                label={t("creator.project.date.clear")}
                onClick={() => void setPublishDate(null)}
              />
            ) : null}
          </div>
        </div>
      ) : null}
      {project?.stage_hint && dismissedHint !== `${project.stage}>${project.stage_hint}` ? (
        // 阶段推进提示（S6c）：只建议，点了才改；「先不用」只收起这一次的这一条。
        <div className="luna-project__hint" role="note">
          <span>{t(`creator.project.hint.${project.stage}`)}</span>
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-btn--sm"
            disabled={savingStage}
            onClick={() => project.stage_hint && void setStage(project.stage_hint)}
          >
            {t("creator.project.hint.move", { stage: t(stageLabelKey(project.stage_hint)) })}
          </button>
          <IconButton
            density="sm"
            icon={<CloseIcon size={12} />}
            label={t("creator.project.hint.dismiss")}
            onClick={() => setDismissedHint(`${project.stage}>${project.stage_hint}`)}
          />
        </div>
      ) : null}

      <div className="luna-project__tabbar">
        <div className="luna-project__tabs" role="tablist" aria-label={t("creator.project.tabsLabel")}>
          {tabs.map((value) => (
            <button
              key={value}
              ref={navigation.register(value)}
              type="button"
              role="tab"
              id={`project-tab-${value}`}
              aria-selected={tab === value}
              aria-controls="project-tabpanel"
              tabIndex={navigation.tabIndexOf(value)}
              className="luna-project__tab"
              onKeyDown={navigation.onKeyDown}
              onClick={() => onPane({ tab: value })}
            >
              {t(`creator.project.tab.${value}`)}
            </button>
          ))}
        </div>
        {pane.tab === "doc" ? (
          // 从项目里点开的一篇：不是固定页签，是一张可关的「正在看」卡片。
          <span className="luna-project__doc" title={pane.path}>
            <span className="luna-project__doc-label">{stemOf(pane.path)}</span>
            <IconButton
              density="sm"
              icon={<CloseIcon size={12} />}
              label={t("creator.project.closeDoc")}
              onClick={() => onPane({ tab: "overview" })}
            />
          </span>
        ) : null}
      </div>

      <div
        className="luna-project__body"
        role="tabpanel"
        id="project-tabpanel"
        aria-labelledby={tab === "doc" ? undefined : `project-tab-${tab}`}
        aria-label={pane.tab === "doc" ? stemOf(pane.path) : undefined}
        data-tab={tab}
      >
        {tab === "overview" ? (
          <div className="luna-project__overview">
            <div className="luna-creator-page__editor">{renderEditor(home, openInProject)}</div>
            <ProjectOverview home={home} materials={materials} onOpenNote={openInProject} onItemMenu={onItemMenu} />
          </div>
        ) : tab === "publish" ? (
          renderPublish({ home, title })
        ) : tab === "distribute" ? (
          <DistributeView home={home} onOpenNote={openInProject} onGoToPublish={() => onPane({ tab: "publish" })} />
        ) : tab === "recording" && project?.script ? (
          <RecordingPanel
            script={project.script}
            createActions={createActions}
            onOpenScriptAt={(line) => {
              const script = project.script;
              if (!script) return;
              requestEditorReveal({ path: script, line, headingIndex: null, focusEditor: true });
              onPane({ tab: "script" });
            }}
            onError={fail}
          />
        ) : tab === "script" && editorPath ? (
          // 脚本工作区（ADR-165 ⑦）：动作在工具栏，事实（段落与时长、引文、灵感）在右侧检查器。
          <div className="luna-script">
            <ScriptToolbar path={editorPath} outline={scriptOutline} onError={fail} />
            <div className="luna-script__main">
              <div className="luna-creator-page__editor">{renderEditor(editorPath, openInProject)}</div>
              <ScriptInspector home={home} path={editorPath} outline={scriptOutline} onOpenNote={openInProject} />
            </div>
          </div>
        ) : editorPath ? (
          <div className="luna-creator-page__editor">{renderEditor(editorPath, openInProject)}</div>
        ) : null}
      </div>
    </section>
  );
}

/** 项目里的一篇笔记该落在哪个页签：主页 → 概览，研究 / 脚本 → 各自页签，其余 → 临时页签。 */
export function paneFor(path: string, home: string, project: ProjectWorkspaceDto | null): ProjectPane {
  if (path === home) return { tab: "overview" };
  if (project?.research === path) return { tab: "research" };
  if (project?.script === path) return { tab: "script" };
  return { tab: "doc", path };
}
