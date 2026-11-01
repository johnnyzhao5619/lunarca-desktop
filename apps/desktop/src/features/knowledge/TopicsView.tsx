/**
 * 「知识库 → 主题页」分区的目录（ADR-088 /
 * TDD-07 §4.1d）。
 *
 * **它是一次查询投影，前端零业务**（ADR-003）：每一行的页名、**确定性一句摘要**、
 * 所有权与**候选来源计数状态**全部由 Rust 的 `topic_directory` 一次下发。本组件不读 frontmatter、
 * 不数建议、不推断所有权，也**不落第二份缓存**——摘要与知识地图是同一条规则、同一个
 * 函数（`luna_vault::topic_summary`），两份实现分叉的表现是同一页在两处各有一句自称的
 * 摘要，而界面上看不出错。
 *
 * **落位在 `features/knowledge/` 而不是 `features/workspace/`**：`check-frontend-structure.mjs`
 * 禁 feature 间成环，当前边向是 `workspace → knowledge`（工作区壳消费知识块），反向引一次
 * 就红。故本组件需要的一切"工作区能力"都以 **props 注入**：行菜单由壳用唯一构建器
 * （`useNoteMenu`，PRD-07-004 / PRD-07-059a）装配好再传进来，本文件不自己拼第二份菜单。
 *
 * **0 页空态是主题页建议的第三个表面**（PRD-16-016 / TDD-07 §4.1c）：它与启动概览、
 * 图谱侧栏共用同一份状态与同一条动作命令，但**恒显**——不承载 `✕`，也不因别处的 `✕`
 * 而消失。`✕` 消掉的只是那两枚主动冒出来的提醒；分区空态是用户主动走进来才看见的解释，
 * 一并消掉等于让这个分区从此答不出「这里为什么是空的」。三态判据全在 Rust。
 *
 * **目录即地图**（PRD-16-061 / ADR-136 ⑥）：地图文件里有的——分组、一句摘要、「最近更新」、
 * 相关主题——目录里都有，且取自同一组 Rust 函数（`group_topics` / `topic_summary` /
 * `recent_topics` / `related_topics`），所以不再另钉一行「知识地图」：那等于给同一份内容的
 * 文件形态开第二扇门。地图文件本身降为页头「…」菜单里的两项（用默认应用打开 / 在访达中显示）。
 */

import {
  Fragment,
  useCallback,
  useEffect,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
} from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  type TopicDirectoryDto,
  type TopicDirectoryGroupDto,
  type TopicDirectoryRowDto,
} from "../../ipc/bindings";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { EmptyState } from "../../components/ui/EmptyState";
import { IndexingNotice } from "../../components/ui/IndexingNotice";
import { QueryBox } from "../../components/ui/QueryBox";
import { IconButton } from "../../components/ui/IconButton";
import {
  ActivityIcon,
  BookOpenIcon,
  EllipsisIcon,
  ExternalWindowIcon,
  FolderIcon,
  SparklesIcon,
  UserIcon,
} from "../../components/ui/Icon";
import { useContextMenu } from "../../components/ContextMenuHost";
import { buildMenu } from "../../lib/context-menu";
import { formatRelativeTime } from "../../lib/format-relative-time";
import { noteStem } from "../../lib/note-path";
import { TopicSuggestionRow } from "./TopicSuggestionRow";
import { requestGraphRescan } from "./graph-scope-menu";
import { TOPIC_DIRECTORY_STALE_EVENT } from "./topic-events";
import { TopicUpgradeDialog } from "./TopicUpgradeDialog";
import { useTopicDraftsSettled } from "./use-topic-drafts-settled";

/** 写盘风暴里重取一次就够（与知识地图入口、图谱控制器同一档位）。 */

type LoadState = "loading" | "ready" | "error";

export interface TopicsViewProps {
  /** 打开这一页（经壳层路由闸口落回本分区并选中，ADR-096 ③；D2 起主区阅读视图渲染）。 */
  onOpen: (path: string) => void;
  /** 当前选中的产物路径（深链与路由闸口设置，壳层持有）。 */
  selectedPath?: string | null;
  /**
   * 行菜单：由工作区壳用**唯一构建器**装配（笔记条目集 + 本页专有动作的能力开关，
   * PRD-07-059a）。本组件只负责"在这一行上把它打开"。
   */
  onRowMenu: (event: ReactMouseEvent, row: TopicDirectoryRowDto) => void;
  /** 回到知识库首页（未选中态的工作台）。首行常驻：选中一页后别无回去的路。 */
  onOpenHome: () => void;
}

export function TopicsView({
  onOpen,
  selectedPath = null,
  onRowMenu,
  onOpenHome,
}: TopicsViewProps) {
  const { t, i18n } = useTranslation("workspace");
  const contextMenu = useContextMenu();
  const [directory, setDirectory] = useState<TopicDirectoryDto | null>(null);
  /** 地图文件的路径（只用于页头「…」菜单）。不存在/多份冲突都由 Rust 回 `null`，菜单不出现——
      闭环仍在概览推荐卡。 */
  const [mapPath, setMapPath] = useState<string | null>(null);
  /** 地图文件动作失败时的一句话（就地给出，不吞掉）。 */
  const [mapError, setMapError] = useState(false);
  const [state, setState] = useState<LoadState>("loading");
  const [filterText, setFilterText] = useState("");
  /** 仍是旧格式的主题页数（Rust 列出，含超出一批上限的部分）；0 = 不给批量升级入口。 */
  const [legacyCount, setLegacyCount] = useState(0);
  const [upgradeOpen, setUpgradeOpen] = useState(false);
  const disposedRef = useRef(false);

  const load = useCallback(() => {
    void commands.topicUpgradeCandidates().then((result) => {
      if (!disposedRef.current) {
        setLegacyCount(result.status === "ok" ? result.data.pages.length + result.data.omitted : 0);
      }
    });
    void commands.knowledgeMapEntry().then((result) => {
      if (!disposedRef.current) setMapPath(result.status === "ok" ? result.data : null);
    });
    void commands.topicDirectory().then((result) => {
      if (disposedRef.current) return;
      if (result.status === "ok") {
        setDirectory(result.data);
        setState("ready");
      } else {
        setState("error");
      }
    });
  }, []);

  useEffect(() => {
    disposedRef.current = false;
    load();
    return () => {
      disposedRef.current = true;
    };
  }, [load]);

  // 建页、归入、正文编辑、重命名、删除都以写盘告终，故跟着 `vault://changed` 重取，
  // 不另立一套"主题页变了"的信号源（那会是同一件事的第二个事实）。合并同一阵变更的
  // 实现只有一份（`use-vault-reload`）——这里此前是自己抄的第三份 `setTimeout`。
  useVaultReload(load);
  // 忽略 / 否决只改随库决定、不动文件：`vault://changed` 不会来，候选数由面板通知重取。
  useEffect(() => {
    window.addEventListener(TOPIC_DIRECTORY_STALE_EVENT, load);
    return () => window.removeEventListener(TOPIC_DIRECTORY_STALE_EVENT, load);
  }, [load]);

  // 后台初稿（PRD-16-060）只写 sidecar，`vault://changed` 不会来：0 页空态的建议行由它重取。
  useTopicDraftsSettled(load);

  // ── 加载失败（PRD-07-023）：给重试，不是一片空白。两个新分区都必须有这一格，
  //    否则拆分就比拆分前倒退了（PRD-07-024）。
  if (state === "error") {
    return (
      <div className="luna-topics">
        <EmptyState
          icon={<BookOpenIcon size={32} />}
          title={t("topics.loadError")}
          action={
            <button type="button" className="luna-btn luna-btn--secondary" onClick={load}>
              {t("common:action.retry")}
            </button>
          }
        />
      </div>
    );
  }

  if (state === "loading" || directory === null) {
    return (
      <p className="luna-empty" role="status">
        {t("common:status.loading")}
      </p>
    );
  }

  const rows = directory.rows;
  const titleOf = new Map(rows.map((row) => [row.path, row.title]));
  const now = Date.now();
  const needle = filterText.trim().toLowerCase();
  const visible = needle
    ? rows.filter(
        (row) =>
          row.title.toLowerCase().includes(needle) || row.summary.toLowerCase().includes(needle),
      )
    : rows;

  return (
    <section className="luna-topics">
      <header className="luna-topics__header">
        {/* 名字由二级分区导航给（PRD-07-065）；这里只报**此刻列表里有几页**——
            过滤之后数字跟着收窄，它答的是"这一屏有几条"而不是"库里有几页"。 */}
        <span className="luna-count-badge">{visible.length}</span>
        {/* 0 页时没有可过滤的对象，摆一个空框只是让空屏多一片说不清的区域。 */}
        {rows.length > 0 && (
          <QueryBox
            className="luna-topics__query"
            value={filterText}
            onChange={setFilterText}
            placeholder={t("topics.filterPlaceholder")}
            ariaLabel={t("topics.filterPlaceholder")}
          />
        )}
        {/* 地图文件的次要入口（PRD-16-061）：目录就是地图，这里只给「文件本身」的两个动作。 */}
        {mapPath !== null && (
          <IconButton
            label={t("topics.mapMenu")}
            icon={<EllipsisIcon size={14} />}
            onClick={(event) => {
              const report = (result: { status: string }) => setMapError(result.status === "error");
              contextMenu.open(
                event,
                buildMenu([
                  {
                    kind: "action",
                    id: "openMap",
                    label: t("topics.mapOpenExternal"),
                    icon: <ExternalWindowIcon size={13} />,
                    onSelect: () => void commands.openVaultPath(mapPath).then(report),
                  },
                  {
                    kind: "action",
                    id: "revealMap",
                    label: t("common:menu.revealInFileManager"),
                    icon: <FolderIcon size={13} />,
                    onSelect: () => void commands.revealVaultPath(mapPath).then(report),
                  },
                ]),
                { highlight: null, label: t("topics.mapMenu") },
              );
            }}
          />
        )}
        {/* 批量升级旧主题（TDD-06 §3.9.5 选择范围）：有旧格式页才出现，单页入口仍在阅读视图。 */}
        {legacyCount > 0 && (
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-btn--sm"
            onClick={() => setUpgradeOpen(true)}
          >
            {t("topicUpgrade.open", { count: legacyCount })}
          </button>
        )}
      </header>
      {/* 开库那一轮扫描期间目录读的是半截索引（V1b）：说出来，扫描完由 useVaultReload 重取。 */}
      <IndexingNotice />
      {mapError && (
        <p className="luna-field-error" role="alert">
          {t("topics.mapActionFailed")}
        </p>
      )}
      {upgradeOpen && (
        <TopicUpgradeDialog
          onClose={() => {
            setUpgradeOpen(false);
            load();
          }}
        />
      )}

      {rows.length === 0 ? (
        <TopicsEmptyState empty={directory.empty} />
      ) : visible.length === 0 ? (
        /* 「这个库还没有主题页」与「没有主题页匹配你输入的词」是两句不同的话
           （PRD-07-021 / PRD-07-024），故过滤无结果**绝不**复用上面那一支。 */
        <EmptyState
          icon={<BookOpenIcon size={24} />}
          title={t("topics.noMatch")}
          action={
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              onClick={() => setFilterText("")}
            >
              {t("common:action.reset")}
            </button>
          }
        />
      ) : (
        <ul className="luna-topics__list">
          {/* 首页 = 未选中态的工作台（最近变化与下一步）。它不是一页产物，而是这份目录
              的「没选中任何一页」——给它一行，用户才回得去。 */}
          <li
            className="luna-topics__row luna-topics__row--home"
            aria-current={selectedPath === null || undefined}
          >
            <button type="button" className="luna-topics__main" onClick={onOpenHome}>
              <span className="luna-topics__title">
                <ActivityIcon size={14} />
                {t("topics.home")}
              </span>
              <span className="luna-topics__summary">{t("topics.homeHint")}</span>
            </button>
          </li>
          {/* 「最近更新」与地图同一个函数（`recent_topics`）：只在未过滤时出现——过滤是在找某一页，
              这时把同一页在顶部再列一遍只是噪声。 */}
          {needle === "" && directory.recent.length > 0 && (
            <>
              <li className="luna-topics__group">
                <h3 className="luna-topics__group-title">{t("topics.recentGroup")}</h3>
              </li>
              {directory.recent.flatMap((path) => {
                const row = rows.find((candidate) => candidate.path === path);
                return row
                  ? [
                      <TopicRow
                        key={`recent\u0000${path}`}
                        row={row}
                        selected={selectedPath === path}
                        onOpen={onOpen}
                        onRowMenu={onRowMenu}
                        titleOf={titleOf}
                        now={now}
                        locale={i18n.language}
                      />,
                    ]
                  : [];
              })}
            </>
          )}
          {/* 分组与顺序全由 Rust 下发（与知识地图同一个函数，PRD-16-050）；只有「其他主题」
              一组时不摆一个孤零零的标题。一页可在多组出现，页头计数仍按页去重。 */}
          {groupedRows(directory.groups, visible).map(({ name, rows: members }, _index, all) => (
            <Fragment key={name ?? ""}>
              {(all.length > 1 || name !== null) && (
                <li className="luna-topics__group">
                  <h3 className="luna-topics__group-title">{name ?? t("topics.otherGroup")}</h3>
                </li>
              )}
              {members.map((row) => (
                <TopicRow
                  key={`${name ?? ""}\u0000${row.path}`}
                  row={row}
                  selected={selectedPath === row.path}
                  onOpen={onOpen}
                  onRowMenu={onRowMenu}
                  titleOf={titleOf}
                  now={now}
                  locale={i18n.language}
                />
              ))}
            </Fragment>
          ))}
        </ul>
      )}

    </section>
  );
}

/** 按 Rust 下发的分组取出当前可见的行；过滤后变空的组不出现。 */
function groupedRows(
  groups: TopicDirectoryGroupDto[],
  visible: TopicDirectoryRowDto[],
): { name: string | null; rows: TopicDirectoryRowDto[] }[] {
  const byPath = new Map(visible.map((row) => [row.path, row]));
  return groups
    .map((group) => ({
      name: group.name,
      rows: group.paths.flatMap((path) => {
        const row = byPath.get(path);
        return row ? [row] : [];
      }),
    }))
    .filter((group) => group.rows.length > 0);
}

function TopicRow({
  row,
  selected,
  onOpen,
  onRowMenu,
  titleOf,
  now,
  locale,
}: {
  row: TopicDirectoryRowDto;
  /** 深链/路由闸口选中的那一页（ADR-096 ③；D2 起主区阅读视图渲染它）。 */
  selected: boolean;
  onOpen: (path: string) => void;
  onRowMenu: (event: ReactMouseEvent, row: TopicDirectoryRowDto) => void;
  /** 路径 → 页名（相关主题按目录里的页名显示，与目录同一口径）。 */
  titleOf: Map<string, string>;
  now: number;
  locale: string;
}) {
  const { t } = useTranslation("workspace");
  const ai = row.ownership === "ai_owned";
  const candidate = row.candidate_source_count;
  const candidateLabel =
    candidate.kind !== "not_computed" && candidate.count > 0
      ? t(candidate.kind === "lower_bound" ? "topics.candidateLowerBound" : "topics.candidateExact", {
          count: candidate.count,
        })
      : null;
  // 已准备好的整理建议（Rust 只在有仍然有效的待确认计划时给精确数）。
  const ready = row.ready_item_count;
  const readyLabel =
    ready.kind === "exact" && ready.count > 0 ? t("topics.readyItems", { count: ready.count }) : null;
  // 系统最近一次整理这一页的时刻（Rust 下发；人工改正文不算）；没有就不写这一格。
  const updated = formatRelativeTime(row.updated_ms, now, locale);
  return (
    <li
      className="luna-topics__row"
      aria-current={selected || undefined}
      onContextMenu={(event) => onRowMenu(event, row)}
    >
      <button type="button" className="luna-topics__main" onClick={() => onOpen(row.path)}>
        <span className="luna-topics__title">{row.title}</span>
        {/* 摘要为空 = 这一页还没有可读的正文段落。如实留空，不编一句。 */}
        {row.summary && <span className="luna-topics__summary">{row.summary}</span>}
      </button>
      {/* 右键之外再给一个可聚焦的入口：只留右键等于对键盘用户不存在。两处开的是
          **同一份**菜单（PRD-07-059a 的唯一构建器）。挂标题行右端（清单行惯例位），
          不挤进元信息行——窄侧栏里它曾把那一行挤到换行（真机第八轮）。 */}
      <IconButton
        className="luna-topics__menu"
        label={t("topics.rowMenu", { title: row.title })}
        icon={<EllipsisIcon size={14} />}
        onClick={(event) => onRowMenu(event, row)}
      />
      <span className="luna-topics__meta">
        {/* 所有权用的就是页头徽标那两句话（`topic.ai` / `topic.user`）——同一个事实
            不另起一套措辞。 */}
        <span
          className="luna-topics__owner"
          data-owner={row.ownership}
          title={t(ai ? "topic.aiHint" : "topic.userHint")}
        >
          {ai ? <SparklesIcon size={11} /> : <UserIcon size={11} />}
          {t(ai ? "topic.ai" : "topic.user")}
        </span>
        {updated && (
          <span className="luna-topics__updated" title={t("topics.updatedHint")}>
            {t("topics.updated", { time: updated })}
          </span>
        )}
        {readyLabel ? (
          <span className="luna-topics__pending" title={t("topics.readyItemsHint")}>
            {readyLabel}
          </span>
        ) : (
          candidateLabel && (
            <span className="luna-topics__pending" title={t("topics.candidateHint")}>
              {candidateLabel}
            </span>
          )
        )}
      </span>
      {/* 相关主题（与本页引用了同一篇来源，与地图同一个函数）：可直接跳过去。 */}
      {row.related.length > 0 && (
        <span className="luna-topics__related">
          <span>{t("topics.related")}</span>
          {row.related.map((path) => (
            <button
              key={path}
              type="button"
              className="luna-topics__related-link"
              onClick={() => onOpen(path)}
            >
              {titleOf.get(path) ?? noteStem(path)}
            </button>
          ))}
        </span>
      )}
    </li>
  );
}

/**
 * 0 页空态的三态（PRD-16-016，判据在 Rust）。
 *
 * ③「总开关关着时一个字都不提」在这里是**结构性成立**的：Rust 在开关关闭时根本不产出
 * 「还没有标签的知识笔记」这条发现，于是 `untagged` 这一档取不到，自然落到教学空态。
 * 前端没有第二处判断，也就没有"忘了加这个条件"的可能。
 */
function TopicsEmptyState({ empty }: { empty: TopicDirectoryDto["empty"] }) {
  const { t } = useTranslation("workspace");

  if (empty?.kind === "suggestion") {
    return (
      <EmptyState
        icon={<BookOpenIcon size={32} />}
        title={t("topics.emptyTitle")}
        description={t("topics.emptyHint")}
        action={
          /* 走 `action` 槽而不是 `children`：`children` 跟在动作之后、不进那条
             `justify-content: center` 的规则，于是这一行在全宽主区里被拉满、按钮被推到
             最右缘，与它解释的那句话在视觉上失联（真机可见）。空态的 CTA 要贴着解释
             文字放——那是用户注意力自然落到的位置。 */
          /* `surface={null}` = **这不是一枚主动提醒**：不上报曝光、不承载 ✕。
             它恒显，也不因别处的 ✕ 而消失（PRD-16-016）。 */
          <TopicSuggestionRow
            suggestion={empty.suggestion}
            surface={null}
            onSettled={() => {
              /* 入队即写盘，目录跟着 `vault://changed` 自己重取——这里没有第二份状态要清。 */
            }}
          />
        }
      />
    );
  }

  if (empty?.kind === "untagged") {
    // ② 直接说出这件事，而不是让用户对着一句「还没有主题页」自己去猜为什么簇凑不出来。
    // 文案与处置动作**沿用体检里那条发现自己的**（PRD-16-030 的补类），不另写一份：
    // 同一件事在两处各说一遍，迟早说成两件事。
    return (
      <EmptyState
        icon={<BookOpenIcon size={32} />}
        title={t("checkup.card.untagged.title")}
        description={t("checkup.card.untagged.desc", { count: empty.count })}
        action={
          <button
            type="button"
            className="luna-btn luna-btn--primary"
            onClick={() => requestGraphRescan(null)}
          >
            <SparklesIcon size={14} />
            <span>{t("checkup.action.rescan")}</span>
          </button>
        }
      />
    );
  }

  // ③ 纯教学空态：说清「这里将来会有什么」，不提任何此刻不可用的能力
  // （PRD-16-001 的教学成本由空态承担，且不新增第四个教学时刻）。
  return (
    <EmptyState
      icon={<BookOpenIcon size={32} />}
      title={t("topics.emptyTitle")}
      description={t("topics.emptyHint")}
    />
  );
}
