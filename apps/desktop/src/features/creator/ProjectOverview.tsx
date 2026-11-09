/**
 * 「项目里的东西」（ADR-162 ⑫ / ADR-165 ⑥ / ADR-167 ③，PRD-19-004 / 037 / 044）：项目首页右侧的
 * 面板——一期视频的全部东西按类列出，外加一个往项目里放来源的入口「添加来源…」（文件、网页、
 * 文字、库里的笔记），文件也可以直接拖进这块面板。
 *
 * 项目首页 = 主页笔记就地编辑（左）+ 这个面板（右）。「属于这个项目」怎么判、归哪一类、来源
 * 是什么类型、类内顺序全在 Rust（`creator_project_overview`）；来源还在转写 / 转换时就地标出
 * 状态（处理队列按 `note_rel`）。「素材与来源」还能看一条来源的「来源指南」、勾选几条「写研究简报」
 * （ADR-167 ⑥）。这里只按 Rust 给的顺序分节渲染、发命令（ADR-003）。
 */

import { useEffect, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  type OverviewGroupDto,
  type OverviewItemDto,
  type ProjectOverviewDto,
} from "../../ipc/bindings";
import { EmptyState } from "../../components/ui/EmptyState";
import { EllipsisIcon, FilmIcon, PlusIcon } from "@lunarca/editor-core/Icon";
import { IconButton } from "../../components/ui/IconButton";
import { useVaultReload } from "../../hooks/use-vault-reload";
import type { ContextMenuEntry } from "../../lib/context-menu";
import { formatDateTime } from "../../lib/format-datetime";
import { PROJECT_DROP_ATTR } from "../../lib/project-file-drop";
import { vaultErrorText } from "../../lib/vault-error";
import { noteTaskState } from "../processing/processing-labels";
import { useProcessingQueueSelector } from "../processing/use-processing-queue";
import type { MaterialImport } from "./use-material-import";
import { useProjectDrop } from "./use-project-drop";
import { PageLoading } from "./CreatorPage";
import { BriefNextDialog, ResearchBriefDialog, SourceGuideDialog } from "./ResearchDialogs";

/** 页面上各类的顺序（与 Rust 排好的顺序一致；这里只决定分节标题）。 */
const GROUPS: readonly OverviewGroupDto[] = ["scripts", "sources", "footage", "research", "swipes", "publish"];

export interface ProjectOverviewProps {
  /** 项目主页的库相对路径。 */
  home: string;
  /** 点一篇：在项目工作区里打开它。 */
  onOpenNote: (path: string) => void;
  /**
   * 一行的笔记菜单（右键或「···」；打开、重命名、删除……，宿主组装，ADR-167 ①）。`leading` 是
   * 这一行专有、排在最前的几项（如「移出项目」）。
   */
  onItemMenu: (event: ReactMouseEvent, path: string, leading?: ContextMenuEntry[]) => void;
  /** 添加来源的状态（与页头「下一步」共用一份）。 */
  materials: MaterialImport;
}

/** 这一篇跟项目的关系（主页链到它 / 它链到主页 / 研究笔记链到它 / 在项目文件夹里），按这个顺序给出。 */
function relationKeys(item: OverviewItemDto): string[] {
  const keys: string[] = [];
  if (item.linked_from_home) keys.push("creator.overview.linkedFromHome");
  if (item.links_to_home) keys.push("creator.overview.linksToHome");
  if (item.linked_from_research && !item.in_folder) keys.push("creator.overview.linkedFromResearch");
  if (item.in_folder && item.group !== "sources") keys.push("creator.overview.inFolder");
  return keys;
}

/** 一条来源此刻在不在处理（每行自己按篇订，队列推进时只重渲染这一行）。 */
function SourceStatus({ path }: { path: string }) {
  const { t } = useTranslation("workspace");
  const state = useProcessingQueueSelector((queue) => noteTaskState(queue.items, path));
  if (!state) return null;
  return (
    <span className="luna-overview__status" data-state={state}>
      {t(`creator.overview.status.${state}`)}
    </span>
  );
}

export function ProjectOverview({ home, onOpenNote, onItemMenu, materials }: ProjectOverviewProps) {
  const { t, i18n } = useTranslation("workspace");
  const [overview, setOverview] = useState<ProjectOverviewDto | null>(null);
  /** 取数失败的说明（取到了就清掉）。 */
  const [notice, setNotice] = useState<string | null>(null);
  /** 面板上一个动作（移出项目）的结果；重取不清它。 */
  const [result, setResult] = useState<string | null>(null);
  /** 开着的来源指南（哪一条）。 */
  const [guideFor, setGuideFor] = useState<OverviewItemDto | null>(null);
  const [briefOpen, setBriefOpen] = useState(false);
  const [nextOpen, setNextOpen] = useState(false);
  /** 最近一次取数（后到的旧结果不覆盖新结果）。 */
  const requestRef = useRef(0);

  const load = () => {
    const request = ++requestRef.current;
    void commands.creatorProjectOverview(home).then((reply) => {
      if (request !== requestRef.current) return;
      if (reply.status !== "ok") {
        setNotice(vaultErrorText(reply.error));
        return;
      }
      setNotice(null);
      setOverview(reply.data);
    });
  };

  useEffect(load, [home, materials.changes]);
  // 项目里新建、移入移出、改了链接：重取一次。
  useVaultReload(load);

  const dropRef = useProjectDrop<HTMLElement>((paths) => void materials.importPaths(paths));

  /** 「移出项目」：只拿掉研究笔记里的那一行，文件不动。 */
  const removeFromProject = async (item: OverviewItemDto) => {
    const reply = await commands.creatorUnlinkSource(home, item.path);
    setResult(
      reply.status === "ok" ? t("creator.overview.removedNotice", { name: item.title }) : vaultErrorText(reply.error),
    );
    load();
  };

  const openMenu = (event: ReactMouseEvent, item: OverviewItemDto) => {
    const leading: ContextMenuEntry[] = [];
    if (item.group === "sources") {
      leading.push({
        kind: "action",
        id: "overview_source_guide",
        label: t("creator.overview.sourceGuide"),
        onSelect: () => setGuideFor(item),
      });
    }
    if (item.linked_from_research && !item.in_folder) {
      leading.push({
        kind: "action",
        id: "overview_remove_from_project",
        label: t("creator.overview.removeFromProject"),
        onSelect: () => void removeFromProject(item),
      });
    }
    onItemMenu(event, item.path, leading.length > 0 ? leading : undefined);
  };
  const sources = overview?.items.filter((item) => item.group === "sources") ?? [];

  // 主页自己就在左边编辑，面板里不再列它（Rust 本就不列）。
  const groups = GROUPS.map((group) => ({
    group,
    items: overview?.items.filter((item) => item.group === group) ?? [],
  })).filter(({ items }) => items.length > 0);

  return (
    <aside
      ref={dropRef}
      className="luna-overview__materials"
      aria-labelledby="project-materials-title"
      {...{ [PROJECT_DROP_ATTR]: home }}
    >
      <div className="luna-overview__materials-header">
        <h2 id="project-materials-title">{t("creator.overview.title")}</h2>
        <div className="luna-overview__materials-actions">
          <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={materials.openAdd}>
            <PlusIcon size={12} />
            {t("creator.overview.addSources")}
          </button>
        </div>
      </div>

      {notice ? (
        <p className="luna-creator-page__hint" role="status">
          {notice}
        </p>
      ) : null}
      {result ? (
        <p className="luna-creator-page__hint" role="status">
          {result}
        </p>
      ) : null}
      {materials.notice && !materials.addOpen ? (
        <p className="luna-creator-page__hint" role="status">
          {materials.notice}
        </p>
      ) : null}
      {overview === null && notice === null ? <PageLoading /> : null}
      {overview && groups.length === 0 && overview.exports.length === 0 ? (
        <EmptyState
          icon={<FilmIcon size={24} />}
          title={t("creator.overview.emptyTitle")}
          description={t("creator.overview.emptyHint")}
        />
      ) : null}
      {groups.map(({ group, items }) => (
        <section key={group} className="luna-overview__materials-group" aria-labelledby={`overview-${group}`}>
          <div className="luna-overview__group-head">
            <h3 id={`overview-${group}`}>{t(`creator.overview.group.${group}`)}</h3>
            {group === "sources" ? (
              <div className="luna-overview__group-actions">
                <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={() => setBriefOpen(true)}>
                  {t("creator.overview.writeBrief")}
                </button>
                <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={() => setNextOpen(true)}>
                  {t("creator.overview.fromBrief")}
                </button>
              </div>
            ) : null}
          </div>
          <ul className="luna-creator-page__list">
            {items.map((item) => (
              <li key={item.path} className="luna-overview__item" onContextMenu={(event) => openMenu(event, item)}>
                <button
                  type="button"
                  className="luna-creator-page__item"
                  title={item.path}
                  onClick={() => onOpenNote(item.path)}
                >
                  <span className="luna-creator-page__item-title">{item.title}</span>
                  {item.kind ? <span className="luna-overview__kind">{t(`creator.overview.kind.${item.kind}`)}</span> : null}
                  {item.group === "sources" ? <SourceStatus path={item.path} /> : null}
                  {relationKeys(item).map((key) => (
                    <span key={key} className="luna-overview__relation">
                      {t(key)}
                    </span>
                  ))}
                  <span className="luna-creator-page__item-meta">{formatDateTime(item.modified_ms, i18n.language)}</span>
                </button>
                <IconButton
                  className="luna-overview__more"
                  density="sm"
                  label={t("creator.overview.more", { name: item.title })}
                  icon={<EllipsisIcon size={14} />}
                  aria-haspopup="menu"
                  onClick={(event) => openMenu(event, item)}
                />
              </li>
            ))}
          </ul>
        </section>
      ))}
      {overview && overview.exports.length > 0 ? (
        <section className="luna-overview__materials-group" aria-labelledby="overview-exports">
          <h3 id="overview-exports">{t("creator.overview.exports")}</h3>
          <ul className="luna-creator-page__list">
            {overview.exports.map((path) => (
              <li key={path}>
                <button
                  type="button"
                  className="luna-creator-page__item"
                  title={t("common:menu.revealInFileManager")}
                  onClick={() => void commands.revealVaultPath(path)}
                >
                  <span className="luna-creator-page__item-title">{path.split("/").pop()}</span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      {guideFor ? <SourceGuideDialog path={guideFor.path} title={guideFor.title} onClose={() => setGuideFor(null)} /> : null}
      {briefOpen ? (
        <ResearchBriefDialog
          home={home}
          sources={sources}
          onClose={() => setBriefOpen(false)}
          onCreated={(path) => {
            setBriefOpen(false);
            load();
            onOpenNote(path);
          }}
        />
      ) : null}
      {nextOpen ? (
        <BriefNextDialog
          home={home}
          onClose={() => setNextOpen(false)}
          onTopicSaved={() => {
            setNextOpen(false);
            setResult(t("creator.overview.topicSaved"));
          }}
          onScriptDrafted={(draft) => {
            setNextOpen(false);
            setResult(t(draft.into_script ? "creator.overview.scriptDrafted" : "creator.overview.scriptDraftAside"));
            load();
            onOpenNote(draft.path);
          }}
        />
      ) : null}
    </aside>
  );
}
