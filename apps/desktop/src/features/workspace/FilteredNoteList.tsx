/**
 * 功能视图清单（录音 / 媒体转写 / 标签结果）的共用列表基座。
 *
 * **文件夹分组**（PRD-07-062）：这些清单的语义是「该类型的全部笔记」，条目随使用无限累积
 * 而只能靠搜索找。传入 `rootDir` 即按笔记所在目录折成可折叠的文件夹树——折叠 / 缩进 /
 * 改名 / 文件夹右键统一由 `FolderedItems` 承担（与 AI 对话历史共用一份实现），
 * 本组件只负责"条目行长什么样"。
 *
 * 归属就是笔记在 Vault 里的路径（笔记即文件），不引入第二套"集合 / 分类"事实源。
 *
 * **多选**（PRD-06-059）：行选择走与笔记文件树同一套原语（`use-note-selection.ts`），
 * 本组件只把它接到行上——Cmd/Shift 的语义、连选区间、随可见行收敛的规则一处实现。
 */

import {
  useState,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { IconButton } from "../../components/ui/IconButton";
import { useTranslation } from "react-i18next";
import type { NoteTypeDto, SearchHitDto } from "../../ipc/bindings";
import { DiscIcon, FileIcon, FilmIcon, FolderPlusIcon } from "@lunarca/editor-core/Icon";
import { distillFolderAction } from "../distill/distill-actions";
import { graphScopeSubmenu } from "../knowledge/graph-scope-menu";
import { useGraphTagExclusions } from "../knowledge/use-graph-tag-exclusions";
import { FolderedItems, type RowDragStart } from "./FolderedItems";
import { listHasNothing } from "./note-folder-tree";
import type { ListLoadState } from "./ListLoadMore";
import { useVisibleRowsReport } from "./use-visible-rows";
import type { NoteSelection } from "./use-note-selection";
import { bucketByTime } from "../../lib/time-buckets";

export interface FilteredNoteListProps {
  /**
   * 清单标题。**可缺省**：住在主侧栏时名字由二级分区导航给（PRD-07-065），
   * 主区用法（标签结果）没有那条导航，仍要自己报名字。
   */
  title?: string;
  /** 标题下的说明；由领域视图提供，不在通用列表里猜语义。 */
  subtitle?: string;
  /** 头部权威总数；省略时等于当前已加载条目数。 */
  count?: number;
  /** null = show all types with generic icon */
  noteType: NoteTypeDto | null;
  items: SearchHitDto[];
  loadState: ListLoadState;
  selectedPath: string | null;
  onSelect: (path: string) => void;
  /**
   * 多选（PRD-06-059）：由清单的持有者给（它同时持有 `selectedPath`，两者必须同源）。
   *
   * **不做成可选**：可选的表现是"录音里能多选、标签结果里不能"，而它们是同一个基座
   * 渲染的同一种对象——本组件存在的理由就是不让这种分叉长出来。
   */
  selection: NoteSelection;
  /** 可见行上报（连选与方向键的取值域，见 `FolderedItems.onVisibleRows`）。 */
  onVisibleRows?: (paths: string[]) => void;
  /** 头部右侧动作区（如「开始录音」主按钮）。 */
  action?: ReactNode;
  /** 标题行右端的轻量工具（如标签结果的「在搜索中查找」）：查询细化类导航，
      不与产出动作同排——量级不同（PRD-06-033 的轻重裁决）。 */
  tools?: ReactNode;
  /** 空态自定义内容（缺省为通用「暂无条目」文案）。 */
  emptyState?: ReactNode;
  /** 行尾徽标（如转写列表的时长）；返回 `null` 即不渲染。列表基座不猜内容，由调用方给。 */
  itemBadge?: (item: SearchHitDto) => string | null;
  /** 标题下的次要元数据（如标签结果的路径与修改时间）。 */
  itemMeta?: (item: SearchHitDto) => ReactNode;
  /**
   * 时间分组（PRD-07-007）：给出每一行用来分桶的时间（毫秒），列表按今天 / 昨天 / 本周 / 更早
   * 分节。只在按时间排序时给——按名称 / 时长排时切成时间段是自相矛盾的。文件夹内条目不参与。
   */
  groupTime?: (item: SearchHitDto) => number | null;
  /** 头部与列表之间的诚实提示（如结果触顶）。 */
  notice?: ReactNode;
  /** 领域级排版钩子；不承载视觉值。 */
  className?: string;
  /**
   * 行右键菜单（条目集由调用方按视图给；缺省则落到宿主兜底，PRD-07-059）。
   *
   * `paths` = 这一次动作真正作用的那一批（右键落在选中集内即整批，落在集外即这一条）。
   * **由这里同拍算好交出去**：调用方自己去读选择集会读到上一批——选择的更新与菜单的
   * 装配发生在同一拍里（`NoteSelection.targetsFor`）。
   */
  onItemContextMenu?: (event: ReactMouseEvent, item: SearchHitDto, paths: readonly string[]) => void;
  /**
   * 该类型的归档目录（Vault 相对，来自 Rust `note_type_archive_dir`）。
   * 传入即启用文件夹分组；`undefined` 保持平铺。
   */
  rootDir?: string;
  /** 归档目录下真实存在的子目录（含空目录），来自 `useNoteFolders`。 */
  dirs?: readonly string[];
  /** 新建文件夹（在归档目录下）；给了才渲染头部那枚按钮。 */
  onCreateFolder?: (name: string) => void;
  /** 重命名文件夹（`path` = 旧的 Vault 相对路径，`name` = 新的末级名）。 */
  onRenameFolder?: (path: string, name: string) => void;
  /** 删除文件夹（走 Vault 删除策略，由调用方确认）。 */
  onDeleteFolder?: (path: string) => void;
  /** 拖进分节：把一批条目移进某个目录（`useNoteFolders.moveInto`）。 */
  onMoveInto?: (targetDir: string, paths: readonly string[]) => void;
  /** 调用方是否正在筛选（有查询）——为真时不分组。 */
  filtering?: boolean;
  /**
   * 分页列尾（TDD-07 §4.1a）：还有下一页时给「加载更多」，取数失败时给重试。
   *
   * **文件夹分组只认已加载的那几页**——归属是笔记的真实路径（笔记即文件），故一个
   * 目录会随着页的加载逐步长出条目。这不是分组算错了，是列表本来就还没到齐；把它藏起来
   * 需要先把整份列表拉完，而那正是这次要修掉的固定上限。
   */
  footer?: ReactNode;
}

export function FilteredNoteList({
  title,
  subtitle,
  count,
  noteType,
  items,
  loadState,
  selectedPath,
  onSelect,
  selection,
  onVisibleRows,
  action,
  tools,
  emptyState,
  itemBadge,
  itemMeta,
  groupTime,
  notice,
  className,
  onItemContextMenu,
  rootDir,
  dirs,
  onCreateFolder,
  onRenameFolder,
  onDeleteFolder,
  onMoveInto,
  filtering = false,
  footer,
}: FilteredNoteListProps) {
  const { t } = useTranslation("workspace");
  const [creating, setCreating] = useState(false);
  /** 文件夹右键「AI 关系标签 ▸」（PRD-06-035c，与笔记树/对话历史同一构建器）。 */
  const tagExclusions = useGraphTagExclusions();

  const Icon = noteType === "recording" ? DiscIcon : noteType === "transcript" ? FilmIcon : FileIcon;

  /** 没有分节的平铺态：行不可拖。 */
  const noDrag: RowDragStart = () => {};

  function renderItem(item: SearchHitDto, beginDrag: RowDragStart) {
    const badge = itemBadge?.(item);
    const meta = itemMeta?.(item);
    return (
      // `role="none"`：行的语义在按钮上（`option`），`<li>` 只是排版容器。
      <li key={item.path} role="none">
        <button
          type="button"
          className="luna-filtered-list__item-btn"
          // 可多选的清单：`option` + `aria-selected` 说"这一批被选中"，`aria-current`
          // 说"这一条正开着"（裁决 D5，两个问题不同）。前者由 `rowProps` 贴，方向键与
          // 连选由容器兑现。
          role="option"
          {...selection.rowProps(item.path)}
          aria-current={selectedPath === item.path || undefined}
          // 拖进分节：选中集里含本条就整批一起拖（与文件树同一规则）。
          onPointerDown={(event) =>
            beginDrag(event, selection.contains(item.path) ? selection.ordered : [item.path])
          }
          onClick={(event) => {
            // Cmd/Ctrl 加选、Shift 连选都**不打开**：每加选一条就把右侧工作面切过去，
            // 用户根本选不满三条（PRD-06-059）。
            if (selection.click(item.path, event)) {
              onSelect(item.path);
            }
          }}
          onContextMenu={(event) => {
            // 右键落在选中集里就保持整批，落在集外则单选它——菜单动作与看到的高亮必须
            // 是同一批对象。落在集外时顺带把工作面切过去（与左键单选同义）。
            const targets = selection.targetsFor(item.path);
            selection.contextClick(item.path);
            if (!selection.contains(item.path)) {
              onSelect(item.path);
            }
            onItemContextMenu?.(event, item, targets);
          }}
          title={item.path}
        >
          <Icon size={14} className="luna-filtered-list__item-icon" />
          <div className="luna-filtered-list__item-content">
            <span className="luna-filtered-list__item-title">{item.title}</span>
            {meta && <small className="luna-filtered-list__item-meta">{meta}</small>}
            {item.snippet && (
              <small className="luna-filtered-list__item-snippet">{item.snippet}</small>
            )}
          </div>
          {badge && <span className="luna-filtered-list__item-badge">{badge}</span>}
        </button>
      </li>
    );
  }

  /** 时间分节：与速记流同一份分桶函数与同一套桶名（`timeBucket.*`）。 */
  const groupRootItems = groupTime
    ? (rows: readonly SearchHitDto[]) =>
        bucketByTime(rows, groupTime, Date.now()).map((bucket) => ({
          id: bucket.id,
          label: t(`common:timeBucket.${bucket.id}`),
          items: bucket.items,
        }))
    : undefined;

  const foldersEnabled = rootDir !== undefined;
  /**
   * 可见行的上报**只有一个来源**：分组在场时是 `FolderedItems`（只有它知道折叠态），
   * 其余两种情形（不分组、空列表——此时它压根不渲染）由这里报。两个分支互斥。
   */
  const groupingRows = foldersEnabled && items.length > 0;
  useVisibleRowsReport(groupingRows ? null : items.map((item) => item.path), onVisibleRows);
  /** 整份清单的多选接线：角色、可多选声明与方向键，只挂在最外层那个 `<ul>` 上。 */
  const rowsProps = {
    role: "listbox",
    "aria-multiselectable": true,
    "aria-label": title ?? t("main.noteList"),
    ...selection.containerProps,
  };

  return (
    <section
      className={["luna-filtered-list", className].filter(Boolean).join(" ")}
      data-note-type={noteType ?? "all"}
      aria-busy={loadState.loading}
    >
      <header className="luna-filtered-list__header luna-sidebar-header">
        <div className="luna-sidebar-header__title">
          {title ? <h1>{title}</h1> : null}
          {(items.length > 0 || (loadState.exhausted && !loadState.loading && !loadState.failed)) && (
            <span className="luna-count-badge">{count ?? items.length}</span>
          )}
          {/* 与「笔记」文件树的头部同一形态（同一个类、同一枚图标、同一条文案）：
              两处都是"在这份清单里新建文件夹"，没有理由长得不一样。 */}
          {foldersEnabled && onCreateFolder && (
            <IconButton
              label={t("tree.newFolder")}
              icon={<FolderPlusIcon size={14} />}
              onClick={() => setCreating(true)}
            />
          )}
          {tools && <span className="luna-filtered-list__tools">{tools}</span>}
        </div>
        {subtitle && <p className="luna-filtered-list__subtitle">{subtitle}</p>}
        {action && <div className="luna-filtered-list__toolbar">{action}</div>}
      </header>
      {notice}
      {/* 空态只在**真的**没东西时出现（`listHasNothing`）：有目录就渲染树；筛选中零命中仍是空态。 */}
      {loadState.exhausted && !loadState.loading && !loadState.failed && listHasNothing({
        itemCount: items.length,
        dirCount: dirs?.length ?? 0,
        creating,
        filtering,
      }) ? (
        (emptyState ?? <p className="luna-empty">{t("main.emptyList")}</p>)
      ) : foldersEnabled ? (
        <FolderedItems
          rootListProps={rowsProps}
          onVisibleRows={onVisibleRows}
          items={items}
          rootDir={rootDir}
          dirs={dirs}
          filtering={filtering}
          renderItem={renderItem}
          groupRootItems={groupRootItems}
          listClassName="luna-filtered-list__items"
          onRenameFolder={onRenameFolder}
          onDeleteFolder={onDeleteFolder}
          onMoveInto={onMoveInto}
          folderMenuExtras={(path) => [
            graphScopeSubmenu(path, tagExclusions, t),
            // 速记的「整理入库」不在这里：速记清单已升格为按时间桶呈现的记录收件箱
            // （PRD-06-058），那里不认识文件夹，故该入口随对象搬去了文件树（`NoteTree`）。
            distillFolderAction(path, t),
          ]}
          creating={creating}
          onCreateFolder={onCreateFolder}
          onCreateCancel={() => setCreating(false)}
        />
      ) : (
        <ul className="luna-filtered-list__items" {...rowsProps}>
          {groupRootItems
            ? groupRootItems(items).flatMap((group) => [
                <li key={`group-${group.id}`} role="none" className="luna-filtered-list__group">
                  <h2 className="luna-filtered-list__group-title">{group.label}</h2>
                </li>,
                ...group.items.map((item) => renderItem(item, noDrag)),
              ])
            : items.map((item) => renderItem(item, noDrag))}
        </ul>
      )}

      {footer}
    </section>
  );
}
