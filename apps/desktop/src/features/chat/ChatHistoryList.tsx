/**
 * 历史会话列表（PRD-03-013）：workspace 侧栏与 panel 弹层共用同一渲染（DRY）。
 * 条目 = Vault `type: chat` 笔记；当前会话按链接路径高亮；hover 操作：删除（走 Vault
 * 删除策略）、在笔记库打开（仅主窗口，`luna:navigate` 单一导航语义）。
 *
 * **行右键菜单由宿主装配**（`useNoteMenu`，PRD-07-004）：本文件此前自己拼了一份，其中
 * 五项（在笔记库打开 / 重命名 / 复制路径 / 删除 / 生成知识笔记）与那个唯一构建器的语义
 * 和 i18n key 逐字相同。会话归档就是库里的一篇 Markdown——同一条笔记在对话历史与文件树
 * 里能做的事不该悄悄分叉，而那正是复制一份菜单的必然结局。
 *
 * **多选**（PRD-06-059）：与笔记树 / 录音 / 转写 / 标签结果 / 速记同一套原语
 * （`use-note-selection.ts`）。会话归档也是库里的一篇 Markdown，"选几篇一起删 / 一起
 * 蒸馏 / 一起拖进文件夹"在这里同样成立——此前这一条清单是七个空间里唯一连加选都没有的
 * （真机 2026-09-09）。
 */

import { useState, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { type ChatHistoryEntryDto } from "../../ipc/bindings";
import { FolderPlusIcon, MessageIcon, NotebookIcon, PlusIcon, TrashIcon } from "@lunarca/editor-core/Icon";
import { CaptureStartButton } from "../../components/ui/CaptureStartButton";
import { IconButton } from "../../components/ui/IconButton";
import { EmptyState } from "../../components/ui/EmptyState";
import { formatHistoryTime } from "./use-chat-history";
import { FolderedItems, type RowDragStart } from "../workspace/FolderedItems";
import { distillFolderAction } from "../distill/distill-actions";
import { graphScopeSubmenu } from "../knowledge/graph-scope-menu";
import { useGraphTagExclusions } from "../knowledge/use-graph-tag-exclusions";
import type { NoteFolders } from "../workspace/use-note-folders";
import type { NoteSelection } from "../workspace/use-note-selection";
import { useVisibleRowsReport } from "../workspace/use-visible-rows";
import { listHasNothing } from "../workspace/note-folder-tree";
import type { ListLoadState } from "../workspace/ListLoadMore";
import { openNote } from "../../lib/navigate";

export interface ChatHistoryListProps {
  loadState: ListLoadState;
  entries: ChatHistoryEntryDto[];
  /** 当前会话链接的笔记路径（高亮）。 */
  activePath: string | null;
  /** 「在笔记库打开」入口（仅主窗口变体提供）。 */
  canOpenInNotes: boolean;
  /**
   * 全库会话**总数**（页头徽标）——不是 `entries.length`。
   *
   * 两者在满页时会分道扬镳：清单是分页的，而徽标承诺的是"一共有几场"
   * （PRD-07-071 ⑦ 与另外四条侧栏同一条规则）。
   */
  total: number;
  /** 列尾的「加载更多」（缺省即不给——面板弹层里那是个临时浮层，不做长列表遍历）。 */
  footer?: ReactNode;
  /**
   * 「新对话」的全局快捷键（已格式化）；空串 = 未绑定，不显示。
   *
   * 面板弹层不传——那扇窗本身就是那条快捷键叫出来的，在它里面再提一次是废话。
   */
  shortcut?: string;
  onNewChat: () => void;
  onOpen: (path: string) => void;
  /** 删除这一批（行内按钮给；右键菜单里的删除归共享构建器）。确认与收尾归调用方。 */
  onDelete: (paths: readonly string[]) => void;
  /**
   * 行右键（PRD-07-059）：条目集由宿主用共享构建器装配。
   *
   * `targets` 是这一次动作真正作用的那一批——右键落在选中集里就是整批，**必须与
   * `contextClick` 同步给出**（见 `NoteSelection.targetsFor`）。
   */
  onItemContextMenu: (
    event: ReactMouseEvent,
    entry: ChatHistoryEntryDto,
    targets: readonly string[],
  ) => void;
  /**
   * 多选（PRD-06-059）：由清单的持有者给（它同时持有 `activePath`，两者必须同源）。
   *
   * **不做成可选**：可选的表现是"侧栏里能多选、弹层里不能"，而它们是同一份渲染。
   */
  selection: NoteSelection;
  /** 可见行上报（连选与方向键的取值域，见 `FolderedItems.onVisibleRows`）。 */
  onVisibleRows?: (paths: string[]) => void;
  /**
   * 文件夹能力（PRD-07-062）；`undefined` = 保持平铺（panel 弹层变体不给文件夹管理，
   * 那是个临时浮层，不该在里面做归档整理）。
   */
  folders?: NoteFolders;
}

export function ChatHistoryList({
  entries,
  loadState,
  total,
  footer,
  shortcut,
  activePath,
  canOpenInNotes,
  onNewChat,
  onOpen,
  onDelete,
  onItemContextMenu,
  selection,
  onVisibleRows,
  folders,
}: ChatHistoryListProps) {
  const { t, i18n } = useTranslation("chat");
  const { t: tWorkspace } = useTranslation("workspace");
  const [creating, setCreating] = useState(false);
  /** 文件夹右键「AI 关系标签 ▸」（PRD-06-035c，与笔记树/功能清单同一构建器）。 */
  const tagExclusions = useGraphTagExclusions();

  /** 会话行（`FolderedItems` 只管分节与折叠，行长什么样归这里；分节形态下不缩进）。 */
  /** 没有分节的平铺态：行不可拖。 */
  const noDrag: RowDragStart = () => {};

  function renderEntry(entry: ChatHistoryEntryDto, beginDrag: RowDragStart) {
    return (
      // 行主体与行内动作是兄弟而不是父子：按钮不能嵌按钮（嵌了的话键盘走一遍只能走到
      // 外层那颗，读屏念出来的也是外层的名字）。与速记行同一形态。
      <li key={entry.path} role="none" className="luna-chat-history__row">
        <button
          type="button"
          className="luna-chat-history__item"
          // 与另外五条清单同一套：`option` + `aria-selected` 说「这一批被选中」，
          // `aria-current` 说「这一条正开着」。前者由 `rowProps` 贴。
          role="option"
          {...selection.rowProps(entry.path)}
          aria-current={entry.path === activePath || undefined}
          // 拖进分节：选中集里含本条就整批一起拖（与文件树同一规则）。
          onPointerDown={(event) =>
            beginDrag(event, selection.contains(entry.path) ? selection.ordered : [entry.path])
          }
          onClick={(event) => {
            // Cmd/Ctrl 加选、Shift 连选都不打开：每加选一条就把整个对话换掉，用户根本
            // 选不满三条（PRD-06-059）。
            if (selection.click(entry.path, event)) {
              onOpen(entry.path);
            }
          }}
          onContextMenu={(event) => {
            // 右键落在选中集里保持整批，落在集外则单选它——菜单动作与看到的高亮必须是
            // 同一批对象。
            //
            // 这里**刻意不像另外五条清单那样顺带打开**：那五条打开的是一个只读工作面，
            // 而这里打开等于把正在进行的对话整个换掉（连同输入框里没发出去的那句）。
            // 右键的意思是"给我菜单"，不是"换一个会话"。
            const targets = selection.targetsFor(entry.path);
            selection.contextClick(entry.path);
            onItemContextMenu(event, entry, targets);
          }}
        >
          <span className="luna-chat-history__item-title" title={entry.title}>
            {entry.title}
          </span>
          <span className="luna-chat-history__item-time">
            {formatHistoryTime(entry.modified_ms, i18n.language)}
          </span>
        </button>
        <span className="luna-chat-history__item-actions">
            {canOpenInNotes ? (
              <IconButton
                density="sm"
                label={t("history.open_in_notes")}
                icon={<NotebookIcon size={12} />}
                onClick={(event) => {
                  event.stopPropagation();
                  openNote(entry.path);
                }}
              />
            ) : null}
          <IconButton
            density="sm"
            tone="danger"
            label={t("history.delete")}
            icon={<TrashIcon size={12} />}
            onClick={(event) => {
              event.stopPropagation();
              onDelete([entry.path]);
            }}
          />
        </span>
      </li>
    );
  }

  /**
   * 可见行的上报**只有一个来源**：分组在场时是 `FolderedItems`（只有它知道折叠态），
   * 平铺时由这里报。两个分支互斥——与 `FilteredNoteList` 同一条规则、同一份实现。
   */
  const grouping = folders?.rootDir !== undefined && entries.length > 0;
  useVisibleRowsReport(grouping ? null : entries.map((entry) => entry.path), onVisibleRows);

  /** 整份清单的多选接线：角色、可多选声明与键盘契约，只挂在最外层那个 `<ul>` 上。 */
  const rowsProps = {
    role: "listbox",
    "aria-multiselectable": true,
    "aria-label": t("history.listLabel"),
    ...selection.containerProps,
  };

  return (
    <div className="luna-chat-history" aria-busy={loadState.loading}>
      {/* 页头与录音 / 转写 / 速记三条侧栏同一顺序与结构（PRD-07-071 ⑦）：计数行在上（计数 +
          新建文件夹，同住 `__title`），主动作在下；不带标题——空间名已在面包屑里，其余三条
          也都不写。此前这里主动作在上、标题「历史会话」+ 计数在下、新建文件夹另占一侧，
          真机上读到的就是"AI 对话的侧栏跟别的不一样"。 */}
      <header className="luna-sidebar-header">
        <div className="luna-sidebar-header__title">
          {(entries.length > 0 || (loadState.exhausted && !loadState.loading && !loadState.failed)) && (
            <span className="luna-count-badge">{total}</span>
          )}
          {folders !== undefined && (
            <IconButton
              label={tWorkspace("tree.newFolder")}
              icon={<FolderPlusIcon size={14} />}
              onClick={() => setCreating(true)}
            />
          )}
        </div>
      </header>
      {/* 「新对话」是本面板的主动作，与录音/速记/转写三条侧栏的主按钮同一枚组件——它此前
          自绘了一份中性描边钮，四条采集侧栏于是有两种"开始"按钮。 */}
      <CaptureStartButton
        icon={<PlusIcon size={14} />}
        label={t("action.new")}
        shortcut={shortcut}
        onClick={onNewChat}
      />
      {/* 空态只在**真的**没东西时出现：有目录就渲染树，否则零会话时建好的文件夹看不见。 */}
      {loadState.exhausted && !loadState.loading && !loadState.failed && listHasNothing({
        itemCount: entries.length,
        dirCount: folders?.dirs.length ?? 0,
        creating,
      }) ? (
        <EmptyState
          icon={<MessageIcon size={24} />}
          title={t("history.emptyTitle")}
          description={t("history.empty")}
        />
      ) : folders?.rootDir !== undefined ? (
        <FolderedItems
          items={entries}
          rootDir={folders.rootDir}
          dirs={folders.dirs}
          renderItem={renderEntry}
          listClassName="luna-chat-history__list"
          rootListProps={rowsProps}
          onVisibleRows={onVisibleRows}
          onRenameFolder={folders.renameFolder}
          onDeleteFolder={folders.deleteFolder}
          onMoveInto={folders.moveInto}
          folderMenuExtras={(path) => [
            graphScopeSubmenu(path, tagExclusions, tWorkspace),
            distillFolderAction(path, tWorkspace),
          ]}
          creating={creating}
          onCreateFolder={folders.createFolder}
          onCreateCancel={() => setCreating(false)}
        />
      ) : (
        <ul className="luna-chat-history__list" {...rowsProps}>
          {entries.map((entry) => renderEntry(entry, noDrag))}
        </ul>
      )}
      {footer}
    </div>
  );
}
