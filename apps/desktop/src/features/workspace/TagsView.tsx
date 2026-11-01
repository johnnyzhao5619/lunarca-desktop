// 标签视图（PRD-06-033，Obsidian 标签面板对标）在「活动栏 + 主侧栏」结构下拆为两件
// （PRD-07-057）：主侧栏「标签树」`TagTreePanel` + 主区「该标签下笔记」`TagNotesView`；
// 侧栏与主区共用 `useTagBrowser` 控制器，选中、请求去竞态、加载与错误状态只有一份。
// `a/b/c` 嵌套标签仅在前端连成可折叠层级树；父级去重计数与来源归属由 Rust
// `TagSummary` 给出。选中父标签的笔记过滤由后端前缀匹配承担，含全部子孙；输入过滤时为平铺
// 匹配列表。支持外部深链选中
// （tags:// 路由 / 编辑器内联 #tag 点击），自动展开祖先层级。

import {
  useEffect,
  useMemo,
  useState,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { useTranslation } from "react-i18next";
import { commands, type SearchHitDto, type TopicDirectoryRowDto } from "../../ipc/bindings";
import { noteStem } from "../../lib/note-path";
import { IgnoredAiTags } from "./IgnoredAiTags";
import { TagRenameDialog } from "./TagRenameDialog";
import {
  BookOpenIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  CloseIcon,
  CopyIcon,
  FoldIcon,
  PencilIcon,
  SearchIcon,
  SparklesIcon,
  TagsIcon,
} from "../../components/ui/Icon";
import { useContextMenu } from "../../components/ContextMenuHost";
import { buildMenu, separator } from "../../lib/context-menu";
import { FilteredNoteList } from "./FilteredNoteList";
import { useNoteSelection } from "./use-note-selection";
import { EmptyState } from "../../components/ui/EmptyState";
import { QueryBox } from "../../components/ui/QueryBox";
import { ancestorPaths, buildTagTree, type TagTreeNode } from "./tag-tree";
import { requestGraphRescan } from "../knowledge/graph-scope-menu";
import { requestTopicDraft } from "../knowledge/topic-draft";
import { TAG_MERGE_EVENT, type TagMergeDetail } from "../../lib/app-events";
import { formatDate, formatDateTime } from "../../lib/format-datetime";
import type { TagBrowserController } from "./use-tag-browser";

/** 收集一个标签节点下所有子孙的路径（整枝展开/折叠用）。 */
function collectDescendants(children: readonly TagTreeNode[]): string[] {
  return children.flatMap((child) => [child.path, ...collectDescendants(child.children)]);
}

function mergeTagIntoNote(tag: string) {
  window.dispatchEvent(new CustomEvent<TagMergeDetail>(TAG_MERGE_EVENT, { detail: { tag } }));
}

/**
 * 发起一次主题页合成，**入队被拒时广播错误**（呈现在标签视图的动作区）。
 * 两个发起点（动作卡 / 树右键菜单）都必须走这里——`requestTopicDraft` 的返回值
 * 一旦被 `void` 丢弃，被拒就成了「点了没反应」（真机第八轮）。
 */
function startTopicDraft(tag: string) {
  // 准备、预览、确认与失败说明都在 `TopicDraftHost` 对话框里，不再入后台队列。
  requestTopicDraft(tag);
}

interface TagTreeRowProps {
  node: TagTreeNode;
  selectedTag: string | null;
  expanded: ReadonlySet<string>;
  onToggleExpand: (path: string) => void;
  onSelectTag: (path: string) => void;
  onContextMenu: (event: ReactMouseEvent, node: TagTreeNode) => void;
  expandLabel: string;
  collapseLabel: string;
  aiOnlyLabel: string;
  aiBadgeLabel: string;
  /** 行内 ✨AI 小标只在**混排**上下文有信息量（PRD-06-046）；分组模式下组头已经
      声明了「AI 关系标签」，组内逐行再标一遍是同一句话说 N 遍（真机第九轮）。 */
  showAiBadge: boolean;
}

export function TagTreeRow({
  node,
  selectedTag,
  expanded,
  onToggleExpand,
  onSelectTag,
  onContextMenu,
  expandLabel,
  collapseLabel,
  aiOnlyLabel,
  aiBadgeLabel,
  showAiBadge,
}: TagTreeRowProps) {
  const hasChildren = node.children.length > 0;
  const isExpanded = expanded.has(node.path);
  const isSelected = selectedTag === node.path;
  return (
    <li>
      <div className="luna-tags-tree__row">
        {hasChildren ? (
          <button
            type="button"
            className="luna-tags-tree__chevron"
            aria-expanded={isExpanded}
            aria-label={isExpanded ? collapseLabel : expandLabel}
            onClick={() => onToggleExpand(node.path)}
          >
            {isExpanded ? <ChevronDownIcon size={12} /> : <ChevronRightIcon size={12} />}
          </button>
        ) : (
          <span className="luna-tags-tree__chevron-placeholder" aria-hidden="true" />
        )}
        <button
          type="button"
          className="luna-tags-list__item"
          aria-current={isSelected || undefined}
          onClick={() => onSelectTag(node.path)}
          onContextMenu={(event) => onContextMenu(event, node)}
        >
          <span className="luna-tags-list__name-group">
            <span className="luna-tags-list__item-name">#{node.name}</span>
            {/* 纯 AI 标签带一枚标记（PRD-06-046）：整支都没有用户亲手打的标签时才标，
                「我也在用的标签」不因 AI 恰好也提到而被降格。 */}
            {node.aiOnly && showAiBadge && (
              <span
                className="luna-tags-list__item-ai"
                title={aiOnlyLabel}
                aria-label={aiOnlyLabel}
              >
                <SparklesIcon size={10} />
                <span>{aiBadgeLabel}</span>
              </span>
            )}
          </span>
          <span className="luna-count-badge">{node.noteCount}</span>
        </button>
      </div>
      {hasChildren && isExpanded ? (
        <ul className="luna-tags-tree__children">
          {node.children.map((child) => (
            <TagTreeRow
              key={child.path}
              node={child}
              selectedTag={selectedTag}
              expanded={expanded}
              onToggleExpand={onToggleExpand}
              onSelectTag={onSelectTag}
              onContextMenu={onContextMenu}
              expandLabel={expandLabel}
              collapseLabel={collapseLabel}
              aiOnlyLabel={aiOnlyLabel}
              aiBadgeLabel={aiBadgeLabel}
              showAiBadge={showAiBadge}
            />
          ))}
        </ul>
      ) : null}
    </li>
  );
}

interface TagTreePanelProps {
  controller: TagBrowserController;
  /** 外部要求选中的标签（nonce 支持重复导航同一标签）：展开其祖先层级。 */
  focusTag?: { tag: string; nonce: number } | null;
  /** 「在搜索中查找」：以 `tag:<标签>` 跳到搜索目的地（PRD-06-031 语法同源）。 */
  onSearchTag?: (tag: string) => void;
  /** 「打开主题页」：落到笔记的编辑工作面（右键里的那条第二路径，PRD-06-033）。 */
  onOpenTopic?: (path: string) => void;
}

/** 主侧栏：嵌套标签树 + 过滤输入（PRD-07-057）。 */
export function TagTreePanel({
  controller,
  focusTag,
  onSearchTag,
  onOpenTopic,
}: TagTreePanelProps) {
  const { t } = useTranslation("workspace");
  const contextMenu = useContextMenu();
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [filterText, setFilterText] = useState("");
  /**
   * 「AI 标签」分组是否收起（PRD-16-043）。视图态、不持久化——它是"这会儿想清净一下"，
   * 不是一项长期配置。
   *
   * **它就是 `PRD-06-046` 那枚「只看用户标签」开关**，不是第二个控件：一个开关加一个
   * 折叠钮做的是同一件事，同屏并存时用户无从判断两者是与关系还是或关系。折叠**只收窄
   * 导航节点**，不改写父标签含全部子孙的过滤语义，也不动任何一个权威笔记总数；被折起来
   * 的标签仍是图谱的边、仍参与 `@` 定范围与 `tag:` 检索（PRD-16-043：这不是隐藏、也不是
   * 降级），故折叠时**不动当前选择**——那是"看不见"，不是"不存在"。
   */
  const [aiCollapsed, setAiCollapsed] = useState(false);
  /** 正在改名/合并的标签（打开预览对话框）。 */
  const [renaming, setRenaming] = useState<string | null>(null);

  useEffect(() => {
    if (focusTag?.tag) {
      // 深链目标可能藏在折叠层级里：展开全部祖先。
      setExpanded((prev) => new Set([...prev, ...ancestorPaths(focusTag.tag)]));
    }
  }, [focusTag]);

  const summaries = controller.summaries;
  /**
   * 按来源分两组（PRD-16-043）：判定是 Rust 下发的**整支** `ai_only`（PRD-06-046），
   * 前端只负责把它们分进两个容器。
   *
   * 分组只在**能力开着**时成立：`vault.graph_ai_tags` 关掉之后，库里既有的 AI 标签
   * 仍然是真标签（仍是图谱的边、仍能被 `tag:` 检索到，PRD-16-043），把它们连同分组一起
   * 拿掉就成了**隐藏**。故关闭时退回一棵不分组的树——「AI 标签分组整组不出现」
   *（PRD-07-024）由此成立，而没有任何一条标签因此消失。
   */
  const grouped = controller.aiTagsEnabled;
  const userTags = useMemo(() => summaries.filter((entry) => !entry.ai_only), [summaries]);
  const aiTags = useMemo(() => summaries.filter((entry) => entry.ai_only), [summaries]);
  const tree = useMemo(() => buildTagTree(grouped ? userTags : summaries), [grouped, summaries, userTags]);
  const aiTree = useMemo(() => buildTagTree(aiTags), [aiTags]);
  // 过滤态：平铺匹配列表（计数降序、同计数按名称）；Obsidian 搜索行为同。
  // **过滤跨两组**：用户在这里问的是"库里有没有这条标签"，而不是"用户组里有没有"。
  const filteredFlat = useMemo(() => {
    if (!filterText) return [];
    return [...summaries]
      .sort((a, b) => b.note_count - a.note_count || a.tag.localeCompare(b.tag))
      .filter((entry) => entry.tag.toLowerCase().includes(filterText.toLowerCase()));
  }, [summaries, filterText]);

  /** 页头徽标的口径：**此刻列表里有几条**——过滤态看匹配数，常规态看全部标签数。 */
  const visibleCount = filterText
    ? filteredFlat.length
    : summaries.filter((entry) => entry.direct_count > 0).length;
  const aiOnlyCount = aiTags.filter((entry) => entry.direct_count > 0).length;

  const toggleExpand = (path: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
      }
      return next;
    });
  };

  /**
   * 标签右键菜单（PRD-07-059）：过滤 / 检索 / 复制 / 改名或合并 / 整枝展开折叠。
   *
   * **改名或合并是一次跨全库的改写**（顶层 `tags` 与正文 `#标签`，ADR-135）：入口只打开
   * 预览对话框——先看影响篇数、是否其实是合并、哪些写法不支持而跳过，确认后逐篇回执、
   * 可整批撤销。不在菜单里「点一下就改」。
   */
  function openTagMenu(
    event: ReactMouseEvent,
    node: { path: string; children?: readonly TagTreeNode[] },
  ) {
    const descendants = collectDescendants(node.children ?? []);
    // **主题页那一项只在这条标签正被选中时给**（PRD-06-033 的分流在右键里的第二条路）：
    // 资格是 Rust 针对**选中标签**下发的事实（`tag_topic_actions`），别的行没有这份事实。
    // 用计数在前端猜一遍就是第二处会漂的定义，而漂了之后菜单里会摆着一项点下去必然落空
    // 的动作（ADR-003 前端零业务）。
    const actions = controller.selectedTag === node.path ? controller.topicActions : null;
    const topicPath = actions?.topic_page ?? null;
    const creatable = actions?.can_create_topic === true ? actions : null;
    contextMenu.open(
      event,
      buildMenu([
        topicPath !== null && {
          kind: "action" as const,
          id: "openTopic",
          label: t("tags.openTopic"),
          icon: <BookOpenIcon size={13} />,
          onSelect: () => onOpenTopic?.(topicPath),
        },
        creatable !== null && {
          kind: "action" as const,
          id: "createTopic",
          label: t("tags.createTopic"),
          icon: <SparklesIcon size={13} />,
          onSelect: () => startTopicDraft(node.path),
        },
        {
          kind: "action",
          id: "filter",
          label: t("tags.filterByTag"),
          icon: <TagsIcon size={13} />,
          onSelect: () => controller.selectTag(node.path),
        },
        {
          // 把这个标签下的笔记汇总成一篇（PRD-06-047）：与「多选汇总」同一内核，
          // 差别只是候选集按标签取。原笔记不删——汇总是新增一份提炼。
          kind: "action" as const,
          id: "mergeTag",
          label: t("tags.mergeIntoNote"),
          icon: <SparklesIcon size={13} />,
          onSelect: () => mergeTagIntoNote(node.path),
        },
        // 全库忽略只给**整支都由 AI 产生**的标签（判定在 Rust 的 `ai_only`）：用户也在用的
        // 标签忽略的只会是其中 AI 那一份，说不清到底忽略了什么（PRD-16-051）。
        summaries.some((entry) => entry.tag === node.path && entry.ai_only) && {
          kind: "action" as const,
          id: "ignoreAiTag",
          label: t("tags.ignoreAiTag"),
          icon: <CloseIcon size={13} />,
          onSelect: () => {
            void commands.setAiTagIgnored(node.path, true);
          },
        },
        {
          kind: "action" as const,
          id: "renameTag",
          label: t("tagRename.menu"),
          icon: <PencilIcon size={13} />,
          onSelect: () => setRenaming(node.path),
        },
        onSearchTag && {
          kind: "action",
          id: "search",
          label: t("tags.searchTag"),
          icon: <SearchIcon size={13} />,
          onSelect: () => onSearchTag(node.path),
        },
        {
          kind: "action",
          id: "copy",
          label: t("tags.copyTag"),
          icon: <CopyIcon size={13} />,
          onSelect: () => {
            void commands.popupCopy(`#${node.path}`);
          },
        },
        descendants.length > 0 ? separator("tree") : null,
        descendants.length > 0 && {
          kind: "action",
          id: "expandChildren",
          label: t("tags.expandChildren"),
          icon: <ChevronDownIcon size={13} />,
          onSelect: () => setExpanded((prev) => new Set([...prev, node.path, ...descendants])),
        },
        descendants.length > 0 && {
          kind: "action",
          id: "collapseChildren",
          label: t("tags.collapseChildren"),
          icon: <FoldIcon size={13} />,
          onSelect: () =>
            setExpanded((prev) => {
              const next = new Set(prev);
              next.delete(node.path);
              for (const path of descendants) {
                next.delete(path);
              }
              return next;
            }),
        },
      ]),
    );
  }

  if (controller.summaryState === "loading" && controller.summaries.length === 0) {
    return <p className="luna-empty">{t("common:status.loading")}</p>;
  }

  if (controller.summaryState === "error") {
    return (
      <EmptyState
        icon={<TagsIcon size={24} />}
        title={t("tags.loadError")}
        action={
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            onClick={controller.refresh}
          >
            {t("common:action.retry")}
          </button>
        }
      />
    );
  }

  return (
    <section className="luna-tags-panel">
      <header className="luna-tags-panel__header luna-sidebar-header">
        {/* 名字由二级分区导航给（PRD-07-065）。 */}
        <div className="luna-sidebar-header__title">
          {/* 计数说的是**此刻列表里有几条**，不是库里一共有几条：它就贴在过滤框上方，
              输入过滤词之后数字随之收窄（与搜索面板同位置的徽标同一口径）。
              折起「AI 关系标签」那一组**不改这个数**——折叠只是把已经在列表里的条目
              收起来，不是把它们排除掉；跟着折叠一起跳动会让人以为标签被过滤没了。 */}
          <span className="luna-count-badge">{visibleCount}</span>
        </div>
        <QueryBox
          value={filterText}
          onChange={setFilterText}
          placeholder={t("tags.filterPlaceholder")}
          ariaLabel={t("tags.filterPlaceholder")}
        />
      </header>
      {controller.summaries.length === 0 ? (
        /* 标题用「还没有标签」而不是面板名——空态的标题该说明当前状况，说一遍面板叫什么
           没有信息量；说明文字用早就写好却**全仓无人使用**的 `tags.emptyHint`，它正是
           这里唯一缺的东西：下一步怎么做才会有标签。 */
        <EmptyState
          icon={<TagsIcon size={24} />}
          title={t("tags.empty")}
          description={t("tags.emptyHint")}
        />
      ) : filterText ? (
        filteredFlat.length === 0 ? (
          <EmptyState
            icon={<TagsIcon size={24} />}
            title={t("tags.noMatch")}
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
          <ul className="luna-tags-list">
            {filteredFlat.map((tag) => (
              <li key={tag.tag}>
                <button
                  type="button"
                  className="luna-tags-list__item"
                  aria-current={controller.selectedTag === tag.tag || undefined}
                  onClick={() => controller.selectTag(tag.tag)}
                  onContextMenu={(event) => openTagMenu(event, { path: tag.tag })}
                >
                  <span className="luna-tags-list__name-group">
                    <span className="luna-tags-list__item-name">#{tag.tag}</span>
                    {tag.ai_only && (
                      <span
                        className="luna-tags-list__item-ai"
                        title={t("tags.aiOnly")}
                        aria-label={t("tags.aiOnly")}
                      >
                        <SparklesIcon size={10} />
                        <span>{t("tags.aiBadge")}</span>
                      </span>
                    )}
                  </span>
                  <span className="luna-count-badge" title={t("tags.countHint")}>
                    {tag.note_count}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )
      ) : (
        (() => {
          const renderTree = (nodes: readonly TagTreeNode[], showAiBadge: boolean) => (
            <ul className="luna-tags-list luna-tags-tree">
              {nodes.map((node) => (
                <TagTreeRow
                  key={node.path}
                  node={node}
                  selectedTag={controller.selectedTag}
                  expanded={expanded}
                  onToggleExpand={toggleExpand}
                  onSelectTag={controller.selectTag}
                  onContextMenu={openTagMenu}
                  expandLabel={t("tags.expand")}
                  collapseLabel={t("tags.collapse")}
                  aiOnlyLabel={t("tags.aiOnly")}
                  aiBadgeLabel={t("tags.aiBadge")}
                  showAiBadge={showAiBadge}
                />
              ))}
            </ul>
          );
          if (!grouped) {
            // 未分组（AI 标签能力关闭等）没有组头声明出处，行内小标是唯一的出处标记。
            return renderTree(tree, true);
          }
          return (
            <>
              <section className="luna-tags-group">
                <h4 className="luna-tags-group__title">{t("tags.groupUser")}</h4>
                {/* 组内空态：一组为 0 不该由整个分区的空态盖掉——「你还没自己打过标签」
                    与「这个库还没有任何标签」是两句不同的话（PRD-07-024）。 */}
                {userTags.length === 0 ? (
                  <p className="luna-tags-group__empty">{t("tags.groupUserEmpty")}</p>
                ) : (
                  renderTree(tree, false)
                )}
              </section>
              <section className="luna-tags-group">
                <h4 className="luna-tags-group__title">
                  <button
                    type="button"
                    className="luna-tags-group__toggle"
                    aria-expanded={!aiCollapsed}
                    onClick={() => setAiCollapsed((current) => !current)}
                  >
                    {aiCollapsed ? <ChevronRightIcon size={12} /> : <ChevronDownIcon size={12} />}
                    <span>{t("tags.groupAi")}</span>
                    <span className="luna-count-badge">{aiOnlyCount}</span>
                  </button>
                </h4>
                {aiCollapsed ? null : aiTags.length === 0 ? (
                  <p className="luna-tags-group__empty">
                    {t("tags.groupAiEmpty")}{" "}
                    <button
                      type="button"
                      className="luna-btn luna-btn--secondary luna-btn--sm"
                      onClick={() => requestGraphRescan(null)}
                    >
                      {t("checkup.action.rescan")}
                    </button>
                  </p>
                ) : (
                  renderTree(aiTree, false)
                )}
                {!aiCollapsed && <IgnoredAiTags />}
              </section>
            </>
          );
        })()
      )}
      {renaming !== null && (
        <TagRenameDialog
          tag={renaming}
          onClose={() => setRenaming(null)}
          onRenamed={(from, to) => {
            // 选中的标签跟到新名字上（改名作用于整支：子标签同样跟过去）。
            const selected = controller.selectedTag;
            if (selected !== null && (selected === from || selected.startsWith(`${from}/`))) {
              controller.selectTag(`${to}${selected.slice(from.length)}`);
            }
          }}
        />
      )}
    </section>
  );
}

/**
 * 标签动作分流（PRD-06-033）：**首选 / 次选按资格渲染，两枚都同屏可见**。
 *
 * **资格判定全在 Rust**（`tag_topic_actions`）并随标签一次下发，前端不按计数自行推导
 * ——「知识笔记」同时需要来源轴与成熟度轴（ADR-086），前端拼一遍就是第二处会漂的定义。
 *
 * **次选绝不折进「更多」、也绝不只留右键**：那样 `PRD-06-047` 承诺的入口就不再成立，
 * 而它是这条标签下最常用的一个动作。
 *
 * **两枚动作名下各有一行产物去向**：「合成主题页」出的是会被持续归入新内容的**活页**，
 * 「汇总成一篇笔记」出的是一次性**快照**。确认卡只承担这一句差异，不作为唯一告知点——
 * 把区别只写在确认卡上，等于要求用户先点错一次才学得会两者不同。
 */
function TagActions({
  tag,
  actions,
  onOpenTopic,
}: {
  tag: string;
  actions: TagBrowserController["topicActions"];
  onOpenTopic?: (path: string) => void;
}) {
  const { t } = useTranslation("workspace");
  const topicRow = useTopicDirectoryRow(actions?.topic_page ?? null);
  const merge = (
    <TagAction
      name={t("tags.mergeIntoNote")}
      hint={t("tags.mergeIntoNoteHint")}
      icon={<SparklesIcon size={14} />}
      onClick={() => mergeTagIntoNote(tag)}
    />
  );

  if (actions?.topic_page) {
    const topicPath = actions.topic_page;
    return (
      <>
        {/* 活页是**目的地**，不是动作（真机第九轮）：此前它长成第三张动作卡，三卡
            并列把浏览页压成一排横幅。改为产物行——页名 + 一句摘要 + 维护状态，点击即
            在知识库打开（NotebookLM 的 artifact 行形态）；动作排只留真正的动作。 */}
        <TagTopicRow path={topicPath} row={topicRow} onOpen={onOpenTopic} />
        {/* 往主题页补内容在那一页上就地进行（「检查可添加内容」，先预览后写入），
            标签页不再另给一个入口（旧版「立即归入」随 ADR-136 退役）。 */}
        {merge}
      </>
    );
  }

  if (actions?.can_create_topic) {
    return (
      <>
        <TagAction
            // 被 ✕ 过的标签：首选**降为普通可用态**但**不消失**——他关掉的是一条
            // 建议，不是一个能力（PRD-16-016）。主次只由 AI 色面表达；「建议」徽标
            // 已收敛为横幅（工作台建议行）独有——那里才有完整语义（可 ✕、有解释），
            // 色面之上再叠一枚徽标是同一句话说两遍（真机第九轮）。
            primary={!actions.dismissed}
            name={t("tags.createTopic")}
            hint={t("tags.livingPageHint")}
            icon={<SparklesIcon size={14} />}
            onClick={() => startTopicDraft(tag)}
          />
        {merge}
      </>
    );
  }

  // 都不符合：只有「汇总成一篇笔记」（PRD-06-047 的入口，逐字保留）。
  return merge;
}

function TagAction({
  name,
  hint,
  icon,
  primary,
  onClick,
}: {
  name: string;
  hint: string;
  icon: ReactNode;
  primary?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      // 首选不是实色主按钮，而是 AI 建议语言的浅紫底（与仪表盘建议带同一语义色面）：
      // 一整块实色底在这排卡里读起来像横幅（真机第八轮）。主次只由色面表达——
      // 「建议」徽标归横幅形态独有（第九轮收敛）。
      className={`luna-btn luna-btn--secondary luna-tag-action${
        primary ? " luna-tag-action--suggested" : ""
      }`}
      onClick={onClick}
    >
      {icon}
      <span className="luna-tag-action__text">
        <span className="luna-tag-action__name">{name}</span>
        {/* 产物去向就在动作名下面这一行——不是 tooltip：禁用/键盘/读屏都读不到 tooltip。 */}
        <span className="luna-tag-action__hint">{hint}</span>
      </span>
    </button>
  );
}

/**
 * 已有主题页时的**产物行**（真机第九轮）：活页是目的地，不是动作——页名 + 一句摘要 +
 * 维护状态，点击即在知识库打开。数据取自主题页目录（与主题页分区同一命令、同一份
 * 标题/摘要/归属事实源，不另抄一份）。
 */
/** 这一页在主题目录里的那一行（页名、摘要、维护状态）；`null` = 取数中、失败或不在目录里。 */
function useTopicDirectoryRow(path: string | null): TopicDirectoryRowDto | null {
  const [row, setRow] = useState<TopicDirectoryRowDto | null>(null);
  useEffect(() => {
    setRow(null);
    if (path === null) return;
    let live = true;
    void commands.topicDirectory().then((result) => {
      if (live && result.status === "ok") {
        setRow(result.data.rows.find((entry) => entry.path === path) ?? null);
      }
    });
    return () => {
      live = false;
    };
  }, [path]);
  return row;
}

function TagTopicRow({
  path,
  row,
  onOpen,
}: {
  path: string;
  row: TopicDirectoryRowDto | null;
  onOpen?: (path: string) => void;
}) {
  const { t } = useTranslation("workspace");
  return (
    <button
      type="button"
      className="luna-tag-topic-row"
      title={t("tags.livingPageHint")}
      onClick={() => onOpen?.(path)}
    >
      <BookOpenIcon size={14} />
      <span className="luna-tag-topic-row__text">
        <span className="luna-tag-topic-row__title">{row?.title ?? noteStem(path)}</span>
        {row !== null && row.summary && (
          <span className="luna-tag-topic-row__summary">{row.summary}</span>
        )}
      </span>
      {/* 维护状态只在取到之后才说：取数中或失败时默认写「Lunarca 维护」，会对一页
          被改过的页说错话。 */}
      {row !== null && (
        <span className="luna-tag-topic-row__owner" data-owner={row.ownership}>
          {t(row.ownership === "ai_owned" ? "topic.ai" : "topic.user")}
        </span>
      )}
    </button>
  );
}

interface TagNotesViewProps {
  controller: TagBrowserController;
  selectedPath: string | null;
  onSelect: (path: string) => void;
  onSearchTag: (tag: string) => void;
  /** 「打开主题页」：经壳层闸口落到「知识库 → 主题页」的只读阅读视图（ADR-096 ③）。 */
  onOpenTopic?: (path: string) => void;
  /**
   * 行右键菜单（笔记条目集由工作区壳给，与其它列表同源）。`paths` = 此刻选中的那一批
   * （多选，PRD-06-059），批量动作作用在它上面。
   */
  onItemContextMenu?: (event: ReactMouseEvent, item: SearchHitDto, paths: readonly string[]) => void;
}

/** 主区：选中标签下的笔记（后端前缀匹配含子孙标签）；未选时提示（PRD-07-057）。 */
export function TagNotesView({
  controller,
  selectedPath,
  onSelect,
  onSearchTag,
  onOpenTopic,
  onItemContextMenu,
}: TagNotesViewProps) {
  const { t, i18n } = useTranslation("workspace");
  const { selectedTag, selectedSummary, summaries, summaryState, noteState, notes } = controller;
  /**
   * 多选（PRD-06-059）与它的可见行。
   *
   * **它住在这里而不是工作区壳里**：选择集该住在"批量动作被渲染的地方"。录音 / 转写
   * 清单的批量删除按钮长在壳给的工具栏上，故那两份住壳里；标签结果没有工具栏，唯一的
   * 批量面就是行右键，故住在这一层——两处都只有一份，不存在第二个事实源。
   */
  const [rows, setRows] = useState<readonly string[]>([]);
  const selection = useNoteSelection({ current: selectedPath, visible: rows });

  if (summaryState === "loading" && summaries.length === 0) {
    return <p className="luna-empty">{t("common:status.loading")}</p>;
  }

  if (summaryState === "error") {
    return (
      <EmptyState
        icon={<TagsIcon size={32} />}
        title={t("tags.loadError")}
        action={
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            onClick={controller.refresh}
          >
            {t("common:action.retry")}
          </button>
        }
      />
    );
  }

  if (summaries.length === 0) {
    return (
      <div className="luna-workspace-placeholder">
        <EmptyState
          icon={<TagsIcon size={32} />}
          title={t("tags.empty")}
          description={t("tags.emptyHint")}
        />
      </div>
    );
  }

  if (!selectedTag || !selectedSummary) return null;

  const emptyState =
    noteState === "loading" ? (
      <p className="luna-empty" role="status">
        {t("tags.loadingNotes")}
      </p>
    ) : noteState === "error" ? (
      <EmptyState
        icon={<TagsIcon size={24} />}
        title={t("tags.notesLoadError")}
        action={
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            onClick={() => controller.selectTag(selectedTag)}
          >
            {t("common:action.retry")}
          </button>
        }
      />
    ) : (
      <EmptyState icon={<TagsIcon size={24} />} title={t("tags.emptyNotes")} />
    );

  return (
    <FilteredNoteList
      className="luna-tag-notes"
      title={`#${selectedTag}`}
      // 「共 N 篇」由头部计数徽标承担（同一个数不说两遍）；这里只留排序口径与出处。
      subtitle={t(selectedSummary.ai_only ? "tags.resultHintAi" : "tags.resultHint")}
      count={selectedSummary.note_count}
      noteType={null}
      items={notes}
      loadState={{ loading: false, failed: false, exhausted: true }}
      selectedPath={selectedPath}
      onSelect={onSelect}
      selection={selection}
      onVisibleRows={setRows}
      onItemContextMenu={onItemContextMenu}
      itemMeta={(item) => {
        // 行副信息瘦身（真机第九轮）：路径只留**目录**（文件名与行标题是同一个词，
        // 整条路径等于把标题念两遍）；时间只留**日期**（列表按修改排序，精确到秒的
        // 时刻在浏览场景里是噪音，悬停 title 仍给全量）。
        const dir = item.path.split("/").slice(0, -1).join("/");
        const modified = formatDate(item.modified_ms, i18n.language);
        return (
          <>
            {dir && <span>{dir}</span>}
            {dir && modified && <span aria-hidden="true">·</span>}
            {modified && (
              <time title={formatDateTime(item.modified_ms, i18n.language)}>{modified}</time>
            )}
          </>
        );
      }}
      // 「在搜索中查找」是查询细化导航，与产出动作不同量级（PRD-06-033 轻重裁决）——
      // 住标题行右端的工具位，不再与动作卡同排。
      tools={
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          onClick={() => onSearchTag(selectedTag)}
        >
          <SearchIcon size={14} />
          <span>{t("tags.searchTag")}</span>
        </button>
      }
      action={
        <div className="luna-filtered-list__actions">
          <TagActions
            tag={selectedTag}
            actions={controller.topicActions}
            onOpenTopic={onOpenTopic}
          />
        </div>
      }
      notice={
        controller.truncated ? (
          <p className="luna-tag-notes__notice">
            {t("tags.truncated", {
              shown: notes.length,
              total: selectedSummary.note_count,
            })}
          </p>
        ) : undefined
      }
      emptyState={emptyState}
    />
  );
}
