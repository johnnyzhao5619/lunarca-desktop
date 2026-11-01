/**
 * 文件夹分组的**呈现基座**（PRD-07-062）：折叠 / 计数 / 行内改名 / 文件夹右键，
 * 一处实现，条目行长什么样由调用方给（`renderItem`）。
 *
 * **形态是「分组」不是「树」**（用户裁决 2026-09-08，对标 Claude 客户端侧栏）：每个
 * 文件夹是清单里的一个**分节**——一行小标题（chevron + 名字 + 条数），其下条目**平铺不缩进**；
 * 嵌套目录也压平成并列的分节，标题用「外 / 内」表示层级。此前这里是带文件夹图标与逐层
 * 缩进的树，与「笔记」文件树同一副长相，用户读到的是"这里也是一棵目录树"，而这些清单
 * 要回答的是"哪些东西是一组的"。实体不变：分组即目录，归属仍是路径。
 *
 * 之所以从列表组件里抽出来：功能视图有两套行样式——速记 / 录音 / 转写是
 * `FilteredNoteList` 的图标 + 标题 + 徽标，AI 对话是 `ChatHistoryList` 的标题 + 时间 +
 * 悬停动作。若各自再写一遍折叠与缩进，很快就会出现"录音里的文件夹能改名、对话里的不能"
 * 这类说不清的差异。
 *
 * **有查询时不分组**（`filtering`）：搜索的意图是"跨文件夹找那一条"，此时把结果拆回
 * 各自的文件夹、还要逐个展开才看得见，恰好抵消了搜索本身。
 */

import {
  useMemo,
  useState,
  type HTMLAttributes,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { useTranslation } from "react-i18next";
import { Fragment } from "react";
import { ChevronDownIcon, ChevronRightIcon, FolderIcon } from "../../components/ui/Icon";
import { useContextMenu } from "../../components/ContextMenuHost";
import { buildMenu, separator, type ContextMenuEntry } from "../../lib/context-menu";
import { cssVariables } from "../../lib/css-variables";
import { noteStem } from "../../lib/note-path";
import { DRAG_GHOST_OFFSET_PX, usePointerDrag } from "../../lib/pointer-drag";
import { canDropInto, groupByFolder, type FolderedItem, type FolderNode } from "./note-folder-tree";
import { useVisibleRowsReport } from "./use-visible-rows";
import { isImeKeyEvent } from "../../lib/ime-key-event";

/**
 * 条目行的起拖入口：调用方把它挂到行元素的 `onPointerDown` 上。`paths` 缺省即本条；
 * 多选清单传整批（选中集里含本条时）。未达阈值不会起拖，单击照常。
 */
export type RowDragStart = (event: React.PointerEvent, paths?: readonly string[]) => void;

export interface FolderedItemsProps<T extends FolderedItem> {
  items: readonly T[];
  /** 该类型的归档目录（Vault 相对，来自 Rust `note_type_archive_dir`）。 */
  rootDir: string;
  /**
   * 归档目录下真实存在的子目录（含**没有任何条目**的那些）。
   *
   * 没有它这棵树就只是条目路径的影子：新建的空目录不会出现，而「新建文件夹」按钮
   * 就在清单顶上（真机 2026-09-08）。
   */
  dirs?: readonly string[];
  /** 调用方正在筛选（有查询）——为真时平铺，见文件头说明。 */
  filtering?: boolean;
  /** 条目行；`beginDrag` 挂到行元素的 `onPointerDown` 上，条目就能被拖进分节。 */
  renderItem: (item: T, beginDrag: RowDragStart) => ReactNode;
  /**
   * 把一批条目移进某个目录（拖到分节标题上松手；「未分组」标题 = 归档目录根）。
   *
   * 拖拽走**指针事件**而非 HTML5 DnD——后者在 Tauri 里 drop 永远不来（`lib/pointer-drag.ts`）。
   * 不给即不可拖（分节只是分组，不承诺搬运）。真机 2026-09-08：「可以创建文件夹，但无法把
   * 已有内容拖过去」——右键「移动到 ▸」在，但分组的心智就是拖进去。
   */
  onMoveInto?: (targetDir: string, paths: readonly string[]) => void;
  /**
   * 根级条目的分节（PRD-07-007 时间分组）：给了就把**不在文件夹里的**条目切成带标题的几段；
   * 文件夹内的条目仍归文件夹——文件夹是用户自己的分组，不被时间桶打散。
   */
  groupRootItems?: (items: readonly T[]) => ReadonlyArray<{ id: string; label: string; items: T[] }>;
  /** 每一层 `<ul>` 的类名（沿用各列表既有样式，不引入第二套列表外观）。 */
  listClassName: string;
  /**
   * 最外层 `<ul>` 的可访问性与键盘接线（多选清单把 listbox 角色与方向键挂在这里）。
   *
   * **只给最外层**：角色与键盘处理器是**整份清单**的属性，每一层各挂一份会让读屏
   * 听见嵌套的三个列表框，方向键也会被内层先吃掉。内层是 `role="group"`（分组容器）。
   */
  rootListProps?: HTMLAttributes<HTMLUListElement>;
  /**
   * 此刻**屏幕上真的有那一行**的条目路径，按渲染顺序（收起的文件夹里那些不在其中）。
   *
   * 折叠态是这个组件的私有状态，而"可见行"是多选连选与方向键的取值域——只有它答得了
   * （`note-selection.ts`：看不见的行不算选中）。故它主动报上去，而不是让调用方按 `items`
   * 猜一遍顺序：分组之后的屏幕顺序是"文件夹在前、散条在后"，与 `items` 并不相同。
   */
  onVisibleRows?: (paths: string[]) => void;
  onRenameFolder?: (path: string, name: string) => void;
  onDeleteFolder?: (path: string) => void;
  /**
   * 文件夹右键的追加条目（如「AI 关系标签 ▸」，PRD-06-035c）。呈现基座不关心
   * 条目语义——由调用方按文件夹路径构建，保持本组件与具体 feature 解耦。
   */
  folderMenuExtras?: (folderPath: string) => readonly ContextMenuEntry[];
  /** 行内新建输入框是否打开（由调用方的「新建文件夹」按钮控制）。 */
  creating?: boolean;
  onCreateFolder?: (name: string) => void;
  onCreateCancel?: () => void;
}

export function FolderedItems<T extends FolderedItem>({
  items,
  rootDir,
  dirs,
  filtering = false,
  renderItem,
  groupRootItems,
  listClassName,
  onRenameFolder,
  onDeleteFolder,
  folderMenuExtras,
  creating = false,
  onCreateFolder,
  onCreateCancel,
  rootListProps,
  onVisibleRows,
  onMoveInto,
}: FolderedItemsProps<T>) {
  const { t } = useTranslation("workspace");
  const { t: tCommon } = useTranslation("common");
  const contextMenu = useContextMenu();
  /** 已折叠的文件夹路径——**默认展开**：折起来会让人以为条目丢了。 */
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set());
  /** 正在改名的文件夹（`null` = 没有）。新建的输入态由调用方经 `creating` 控制。 */
  const [renaming, setRenaming] = useState<{ path: string; draft: string } | null>(null);
  const [createDraft, setCreateDraft] = useState("");

  const grouped = useMemo(
    () => (filtering ? null : groupByFolder(items, rootDir, dirs)),
    [items, rootDir, dirs, filtering],
  );

  // ── 拖进分节 ──────────────────────────────────────────────────────────────
  const drag = usePointerDrag<readonly string[]>((paths, targetDir) => {
    // 松手与高亮同一判据（`canDropInto`）：不合法时高亮本来就没亮，此处再放行等于让用户
    // 看着一个"放不进去"的落点却真的把命令发出去。
    if (!canDropInto(paths, targetDir)) return;
    onMoveInto?.(targetDir, paths);
  });
  const dropTarget =
    drag.payload !== null && drag.dropId !== null && canDropInto(drag.payload, drag.dropId)
      ? drag.dropId
      : null;
  const dragStartFor =
    (item: T): RowDragStart =>
    (event, paths = [item.path]) => {
      if (onMoveInto) drag.begin(event, paths);
    };

  /** 屏幕顺序（收起的文件夹整块跳过）——与下面的渲染同一个遍历顺序。 */
  const visibleRows = useMemo(() => {
    if (grouped === null) {
      return items.map((item) => item.path);
    }
    const out: string[] = [];
    // 与下面的渲染同一顺序：每个分节先自己的条目（收起则跳过），再它的子分节；根级最后。
    const walk = (folders: readonly FolderNode<T>[]) => {
      for (const folder of folders) {
        if (!collapsed.has(folder.path)) {
          out.push(...folder.items.map((item) => item.path));
        }
        walk(folder.folders);
      }
    };
    walk(grouped.folders);
    out.push(...grouped.items.map((item) => item.path));
    return out;
  }, [grouped, items, collapsed]);

  useVisibleRowsReport(visibleRows, onVisibleRows);

  function toggleFolder(path: string) {
    setCollapsed((current) => {
      const next = new Set(current);
      if (!next.delete(path)) {
        next.add(path);
      }
      return next;
    });
  }

  function openFolderMenu(event: ReactMouseEvent, folder: FolderNode<T>) {
    const extras = folderMenuExtras?.(folder.path) ?? [];
    if (onRenameFolder === undefined && onDeleteFolder === undefined && extras.length === 0) {
      return;
    }
    contextMenu.open(
      event,
      buildMenu([
        onRenameFolder !== undefined && {
          kind: "action",
          id: "renameFolder",
          label: tCommon("menu.rename"),
          onSelect: () => setRenaming({ path: folder.path, draft: folder.name }),
        },
        extras.length > 0 && separator("extras"),
        ...extras,
        onDeleteFolder !== undefined && separator("danger"),
        onDeleteFolder !== undefined && {
          kind: "action",
          id: "deleteFolder",
          label: tCommon("menu.delete"),
          danger: true,
          onSelect: () => onDeleteFolder(folder.path),
        },
      ]),
    );
  }

  /** 行内输入框：空名等于取消（比弹一句"名字不能为空"更省事，也更符合直觉）。 */
  function renderEditor(
    value: string,
    onChange: (next: string) => void,
    onCommit: () => void,
    onCancel: () => void,
  ) {
    return (
      <li className="luna-folder-editor">
        <FolderIcon size={14} className="luna-folder-editor__icon" />
        <input
          type="text"
          className="luna-input"
          autoFocus
          value={value}
          placeholder={t("folder.namePlaceholder")}
          aria-label={t("folder.namePlaceholder")}
          onChange={(event) => onChange(event.currentTarget.value)}
          onBlur={onCommit}
          onKeyDown={(event) => {
            if (isImeKeyEvent(event)) return;
            if (event.key === "Enter") {
              onCommit();
            } else if (event.key === "Escape") {
              onCancel();
            }
          }}
        />
      </li>
    );
  }

  function commitRename() {
    const current = renaming;
    setRenaming(null);
    const name = current?.draft.trim() ?? "";
    if (current === null || name === "" || name === current.path.split("/").pop()) {
      return;
    }
    onRenameFolder?.(current.path, name);
  }

  function commitCreate() {
    const name = createDraft.trim();
    setCreateDraft("");
    onCreateCancel?.();
    if (name !== "") {
      onCreateFolder?.(name);
    }
  }

  /** 分节标题：相对归档目录的路径段，嵌套目录以「 / 」相连——分节平铺，不缩进。 */
  function groupLabel(folder: FolderNode<T>): string {
    const prefix = `${rootDir}/`;
    const rel = folder.path.startsWith(prefix) ? folder.path.slice(prefix.length) : folder.path;
    return rel.split("/").join(" / ");
  }

  /** 一个文件夹 = 一个分节：标题行 + 自己的条目（收起则不渲染）+ 子分节（各自独立收放）。 */
  function renderGroup(folder: FolderNode<T>): ReactNode[] {
    const isCollapsed = collapsed.has(folder.path);
    const Chevron = isCollapsed ? ChevronRightIcon : ChevronDownIcon;
    const head =
      renaming?.path === folder.path ? (
        <Fragment key={`folder-${folder.path}`}>
          {renderEditor(
            renaming.draft,
            (draft) =>
              setRenaming((current) => (current === null ? current : { ...current, draft })),
            commitRename,
            () => setRenaming(null),
          )}
        </Fragment>
      ) : (
        // 分节标题只是排版，不是行：`role="none"` 让它不进 listbox 的选项集。
        // 它同时是拖拽落点（`data-drop-id`），拖到它上面松手 = 移进这个目录。
        <li
          key={`folder-${folder.path}`}
          role="none"
          className="luna-filtered-list__group"
          data-drop-id={onMoveInto ? folder.path : undefined}
          data-drop-target={dropTarget === folder.path || undefined}
        >
          <button
            type="button"
            className="luna-folder-group__head"
            aria-expanded={!isCollapsed}
            onClick={() => toggleFolder(folder.path)}
            onContextMenu={(event) => openFolderMenu(event, folder)}
            title={folder.path}
          >
            <Chevron size={12} className="luna-folder-group__chevron" />
            <span className="luna-folder-group__name">{groupLabel(folder)}</span>
            {/* 只数这一节自己的条目：子分节各报各的，合计会让两处数字对不上。 */}
            <span className="luna-folder-group__count">{folder.items.length}</span>
          </button>
        </li>
      );
    return [
      head,
      ...(isCollapsed ? [] : folder.items.map((item) => renderItem(item, dragStartFor(item)))),
      ...folder.folders.flatMap((child) => renderGroup(child)),
    ];
  }

  const dragGhost =
    drag.payload === null ? null : (
      <div
        className="luna-drag-ghost"
        style={cssVariables({
          "--drag-x": `${drag.position.x + DRAG_GHOST_OFFSET_PX}px`,
          "--drag-y": `${drag.position.y + DRAG_GHOST_OFFSET_PX}px`,
        })}
      >
        {/* 拖一批时报数：拖影上只印第一条的名字会让人以为只拖走了那一条。 */}
        {drag.payload.length === 1
          ? noteStem(drag.payload[0])
          : t("tree.dragCount", { count: drag.payload.length })}
      </div>
    );

  return (
    <>
    <ul className={listClassName} {...rootListProps}>
      {/* 新建输入框恒在最上方——包括空列表时：先建文件夹再往里放，是常见顺序。 */}
      {creating &&
        renderEditor(createDraft, setCreateDraft, commitCreate, () => {
          setCreateDraft("");
          onCreateCancel?.();
        })}
      {grouped === null
        ? renderRootItems(items, false)
        : [
            ...grouped.folders.flatMap((folder) => renderGroup(folder)),
            ...renderRootItems(grouped.items, grouped.folders.length > 0),
          ]}
    </ul>
    {dragGhost}
    </>
  );

  /**
   * 根级条目。有分节在场而根级又有条目时，给根级一行「未分组」标题——否则它们看起来
   * 像是挂在最后一个分节底下（Claude 客户端把这一段叫 "More"，同一个问题）。
   */
  function renderRootItems(rows: readonly T[], hasGroups: boolean): ReactNode[] {
    // 「未分组」标题同时是"拖回根目录"的落点：拖拽进行中即便根级没有条目也把它画出来，
    // 否则从分节里拖出来的东西没处放。有时间桶的清单也一样——桶是根级条目**内部**的
    // 分节，「未分组」是它们与文件夹分节之间的那一层（真机 2026-09-08：录音 / 转写 / 速记
    // 都走时间桶，此前这一层只写在了没有桶的分支里，于是只有对话看得见它）。
    const rootDrop = {
      "data-drop-id": onMoveInto ? rootDir : undefined,
      "data-drop-target": dropTarget === rootDir || undefined,
    };
    const ungroupedHeading =
      hasGroups && (rows.length > 0 || drag.payload !== null)
        ? [
            <li key="group-ungrouped" role="none" className="luna-filtered-list__group" {...rootDrop}>
              <h2 className="luna-filtered-list__group-title">{t("folder.ungrouped")}</h2>
            </li>,
          ]
        : [];
    if (!groupRootItems) {
      return [...ungroupedHeading, ...rows.map((item) => renderItem(item, dragStartFor(item)))];
    }
    return [
      ...ungroupedHeading,
      ...groupRootItems(rows).flatMap((group) => [
        // 分节标题只是排版，不是行：`role="none"` 让它不进 listbox 的选项集。
        // 有「未分组」在上时桶标题降一级（`--sub`），并同为"拖回根目录"的落点——整个未分组
        // 区域都能接住，不必对准那一行小字。
        <li
          key={`group-${group.id}`}
          role="none"
          className="luna-filtered-list__group"
          {...(hasGroups ? rootDrop : {})}
        >
          <h2
            className={
              hasGroups
                ? "luna-filtered-list__group-title luna-filtered-list__group-title--sub"
                : "luna-filtered-list__group-title"
            }
          >
            {group.label}
          </h2>
        </li>,
        ...group.items.map((item) => renderItem(item, dragStartFor(item))),
      ]),
    ];
  }
}
