// 笔记文件树（PRD-06-001/006，Obsidian 文件浏览器对标）：
// 右键上下文菜单（条目集与其余视图同源，`note-menu.tsx`）、树内联重命名与新建输入、
// 多选与整批拖拽移动（选择原语与五条清单同源，`use-note-selection.ts`，PRD-06-059）、
// 排序切换（名称/修改时间）。业务落 Rust 命令，本组件只发命令、渲染。

import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { cssVariables } from "../../lib/css-variables";
import { IconButton } from "../../components/ui/IconButton";
import { useTranslation } from "react-i18next";
import { noteTypeLabelKey } from "../../lib/note-type";
import { commands, type NoteTreeDto, type TreeSortDto } from "../../ipc/bindings";
import {
  BookmarkIcon,
  ChevronRightIcon,
  CloudUploadIcon,
  DownloadIcon,
  FileIcon,
  FileTextIcon,
  FilmIcon,
  FilterIcon,
  FolderIcon,
  FolderPlusIcon,
  FoldIcon,
  ImageIcon,
  MusicIcon,
  NotebookIcon,
  PlusIcon,
  SortIcon,
  TemplateIcon,
  TrashIcon,
} from "../../components/ui/Icon";
import { EmptyState } from "../../components/ui/EmptyState";
import { useConfirm } from "../../components/ConfirmHost";
import { useContextMenu } from "../../components/ContextMenuHost";
import { useDeleteConsequence } from "../../hooks/use-delete-mode";
import { buildMenu, separator, type ContextMenuEntry } from "../../lib/context-menu";
import { deleteNotesWithConfirm, useNoteMenu } from "./note-menu";
import {
  ancestorDirsOf,
  containsDraft,
  countEntries,
  dirChoices,
  hideDraftNodes,
  visibleRowPaths,
} from "./note-tree-view";
import { useNoteSelection } from "./use-note-selection";
import { useDownloadPercent, useSyncCloudOnly, type DownloadProgress } from "./use-sync-cloud-only";
import { useTreeViewState } from "./use-tree-view-state";
import { noteStem } from "../../lib/note-path";
import { distillFolderAction } from "../distill/distill-actions";
import { classifyAssetTarget } from "../media/asset-kind";
import { graphScopeSubmenu } from "../knowledge/graph-scope-menu";
import { BookmarksSection, useVaultBookmarks } from "./BookmarksSection";
import { useGraphTagExclusions } from "../knowledge/use-graph-tag-exclusions";
import { DRAG_GHOST_OFFSET_PX, usePointerDrag } from "../../lib/pointer-drag";
import { dropOnEditor, editorSurfaceAt, markEditorDropTarget } from "../../lib/editor-file-drop";
import { TREE_FILE_DROP_EVENT, type TreeFileDrop } from "../../lib/tree-file-drop";
import { SHOW_DRAFTS_EVENT } from "../../lib/app-events";
import { isImeKeyEvent } from "../../lib/ime-key-event";
import { afterEditorsSaved } from "../editor/editor-save-guard";
import { announceRelocation } from "../../lib/app-events";
import { vaultErrorText } from "../../lib/vault-error";

/**
 * 文件节点类型图标（Obsidian 对标：图片/音视频/文档各显其类，PRD-06-007）。
 *
 * 这里**没有**主题页/知识地图分支：知识产物不进笔记树（ADR-096 ②，Rust 建树时过滤），
 * 产物的身份图标归知识库目录承担。
 */
export function FileNodeIcon({ path, className }: { path: string; className?: string }) {
  const kind = classifyAssetTarget(path);
  const props = { size: 14, className } as const;
  switch (kind) {
    case "image":
      return <ImageIcon {...props} />;
    case "audio":
      return <MusicIcon {...props} />;
    case "video":
      return <FilmIcon {...props} />;
    case "pdf":
    case "binary":
      return <FileTextIcon {...props} />;
    default:
      return <FileIcon {...props} />;
  }
}

/** 排序菜单里的三档，顺序即菜单顺序（PRD-06-006 ①）；类型由 Rust 生成（设备状态里存的就是它）。 */
export const TREE_SORT_MODES: readonly TreeSortDto[] = ["name", "mtime", "created"];

/**
 * 「只在云端」徽标的字：下载中报百分比，否则写「仅云端」。
 *
 * 自己订这一行的进度：进度一拍一推，经整棵树走一遍就是每一拍重渲染整棵树。
 */
function CloudOnlyBadgeText({ progress, path }: { progress: DownloadProgress; path: string }) {
  const { t } = useTranslation("workspace");
  const percent = useDownloadPercent(progress, path);
  return <>{percent !== undefined ? t("cloud.sync_progress", { pct: percent }) : t("cloud.sync_badge")}</>;
}

/** 目录置顶 + 名称自然序；时间档下文件按该时间降序（目录仍按名称）。 */
export function sortTreeChildren(
  children: readonly NoteTreeDto[],
  mode: TreeSortDto,
): NoteTreeDto[] {
  return [...children].sort((a, b) => {
    if (a.is_dir !== b.is_dir) {
      return a.is_dir ? -1 : 1;
    }
    if (mode !== "name" && !a.is_dir) {
      const delta =
        mode === "mtime" ? (b.mtime_ms ?? 0) - (a.mtime_ms ?? 0) : (b.created_ms ?? 0) - (a.created_ms ?? 0);
      if (delta !== 0) {
        return delta;
      }
    }
    return a.name.localeCompare(b.name, undefined, {
      numeric: true,
      sensitivity: "base",
    });
  });
}

export function parentPathOf(path: string): string {
  const idx = path.lastIndexOf("/");
  return idx === -1 ? "" : path.slice(0, idx);
}

export function baseNameOf(path: string): string {
  const idx = path.lastIndexOf("/");
  return idx === -1 ? path : path.slice(idx + 1);
}

/**
 * 按路径在树里找那个节点（顶栏动作要作用于**选中的那一个**，就得先拿到它本身）。
 *
 * 存在的理由是「不靠路径字面量猜节点属性」：删除按钮此前用 `!path.endsWith(".md")`
 * 当作"这是个目录"，于是选中任何非 md 附件都会走成删除文件夹的分支。类型是树里那一行
 * 已经写着的事实（`is_dir` 由 Rust 下发），没有理由再从字符串里推一遍。
 * 找不到返回 `null`——调用方据此禁用动作，而不是拿一个猜出来的默认值往下走。
 */
export function findTreeNode(
  nodes: readonly NoteTreeDto[],
  path: string | null,
): NoteTreeDto | null {
  if (!path) {
    return null;
  }
  for (const node of nodes) {
    if (node.path === path) {
      return node;
    }
    // 只下探路径前缀命中的子树（同级目录数十上百时不必逐棵走）。
    if (node.is_dir && path.startsWith(`${node.path}/`)) {
      return findTreeNode(node.children, path);
    }
  }
  return null;
}

/** 拖拽目标合法性：不落回原父目录、不落进自身或自身后代。 */
export function isValidDropTarget(sourcePath: string, targetDir: string): boolean {
  if (!sourcePath) {
    return false;
  }
  if (targetDir === sourcePath) {
    return false;
  }
  if (parentPathOf(sourcePath) === targetDir) {
    return false;
  }
  return !targetDir.startsWith(`${sourcePath}/`);
}

/**
 * 这一批能不能放到该落点——**落点高亮与松手动作共用同一判据**。
 *
 * 两条，各自对应一种"放下去会发生什么"：
 * ① **没有一项是移进自身或自身后代**——那是 Rust 会整批拒绝的错（`plan_moves` 的
 *    `IntoItself`），高亮却亮着的话，用户是在被邀请做一件必然失败的事；
 * ② **至少有一项真的会被搬走**——一批里混着"已经在那儿"的项很正常（跨文件夹多选后
 *    拖回其中一个文件夹），它们由 Rust 按"已达成"跳过，不该因此挡住整次拖拽。
 *
 * 这两条**不能合成"每一项都合法"**：那会让上面 ② 那种再普通不过的选择整批放不下去。
 */
export function canDropSelection(sourcePaths: readonly string[], targetDir: string): boolean {
  if (sourcePaths.length === 0) {
    return false;
  }
  if (sourcePaths.some((path) => !path || targetDir === path || targetDir.startsWith(`${path}/`))) {
    return false;
  }
  return sourcePaths.some((path) => parentPathOf(path) !== targetDir);
}

/** 拖影跟随光标的偏移（px）：略微错开，避免拖影盖住落点高亮。 */

/**
 * **这一次编辑的身份**——注意它里面没有草稿。
 *
 * 草稿曾经就长在这里，而"打开输入框时全选"那个 effect 依赖的是整个对象：于是每敲
 * 一个字符 → 新对象 → effect 重跑 → `select()` 把整串重新选上 → 下一个字符直接覆盖掉
 * 全部内容。表现就是"几乎没法完成一次重命名"（2026-09-02 真机上报）。
 * 拆开之后，"聚焦并全选"的依赖是**这是哪一次编辑**（kind + path），它在一次编辑里恒定。
 */
interface EditingState {
  /** rename = 已有节点改名；create = 在 parent 下新建。 */
  kind: "rename" | "create-note" | "create-folder";
  /** rename：节点路径；create：父目录路径（空串 = 根）。 */
  path: string;
  isDir: boolean;
}

export interface NoteTreeProps {
  tree: NoteTreeDto | null;
  selectedPath: string | null;
  /**
   * 选中/打开；`newTab` = 中键或右键「在新标签页打开」（多标签语义，PRD-07-035）。
   *
   * **Cmd/Ctrl+点击在文件树里改判为"加选"**（PRD-06-059）：文件树是管理文件的地方，
   * 而"选中好几个再一起处理"是这里最基础的动作；新标签仍有中键与右键两条既有入口，
   * 别处（链接、搜索结果）的 Cmd+点击语义一律不变。
   */
  /**
   * 选中 / 打开。单击 = 浏览（预览标签）；`keep` = 双击（明确要留着它，常驻）；`newTab` = 中键 /
   * 右键「新标签打开」（PRD-07-035）。
   */
  onSelect: (path: string | null, options?: { newTab?: boolean; keep?: boolean }) => void;
  /** 顶栏「新建」：沿用类型 + 时间戳自动命名主流程（PRD-06-008）。 */
  onCreateNote: () => void;
  onOpenTemplatePicker: () => void;
  onStatus: (status: string) => void;
  /** 移动/重命名后通知（打开中的标签路径重映射）。 */
  onMoved?: (from: string, to: string) => void;
  /** 删除后通知（关闭指向该路径及其子路径的标签）。 */
  onDeleted?: (path: string) => void;
  /** 在右侧分屏打开（右键菜单，PRD-07-036）。 */
  onOpenInSplit?: (path: string) => void;
  /** 在独立窗口打开（右键菜单，PRD-07-036 弹出窗口）。 */
  onOpenInWindow?: (path: string) => void;
}

export function NoteTree({
  tree,
  selectedPath,
  onSelect,
  onCreateNote,
  onOpenTemplatePicker,
  onStatus,
  onMoved,
  onDeleted,
  onOpenInSplit,
  onOpenInWindow,
}: NoteTreeProps) {
  const { t } = useTranslation("workspace");
  const { t: tCommon } = useTranslation("common");
  /** 删除确认卡里那句「删了之后会怎样」——按当前删除策略取，不再硬写「可从回收站还原」。 */
  const deleteConsequence = useDeleteConsequence();
  const ask = useConfirm();
  /** 文件夹右键「AI 关系标签 ▸」的排除清单（PRD-06-035c）。 */
  const tagExclusions = useGraphTagExclusions();
  const bookmarks = useVaultBookmarks();
  // 排序与展开是设备状态：重启后还原（PRD-06-006、ADR-140）。
  const { expandedDirs, setExpandedDirs, sortMode, setSortMode } = useTreeViewState();
  // 官方同步里只在云端的文件（ADR-156）：按需下载 / 释放本机空间 / 下载进度。
  const cloudOnly = useSyncCloudOnly(onStatus);
  // 同步盘留下的冲突副本（TDD-06 §4.1 ①）：判定在 Rust，树每次刷新跟着重取，行上打一枚标记。
  const [conflictCopies, setConflictCopies] = useState<ReadonlySet<string>>(() => new Set());
  useEffect(() => {
    let active = true;
    void commands.conflictCopies().then((result) => {
      if (active && result.status === "ok") setConflictCopies(new Set(result.data.map((copy) => copy.path)));
    });
    return () => {
      active = false;
    };
  }, [tree]);
  const [editing, setEditing] = useState<EditingState | null>(null);
  /** 输入框里此刻的字（与"这是哪一次编辑"分家，见 `EditingState` 的说明）。 */
  const [editDraft, setEditDraft] = useState("");
  /** 「隐藏待整理」过滤开关（PRD-06-052）：视图态、会话内记忆不持久化——它表达的是
      「这会儿别让草稿分散注意力」这种临时聚焦，**不移动任何文件、不生成虚拟根**。
      **刻意不做成段控**（ADR-087 ① / PRD-06-048 ④）：成熟度会流转，记录会沉淀成知识，
      段控暗示的却是静态归属；而「全部笔记」就该是磁盘的真相。 */
  const [hideDrafts, setHideDrafts] = useState(false);
  /** 渲染用的顶层节点：**真实 Vault 拓扑**（ADR-087 ①），只按「隐藏待整理」临时过滤。 */
  const rendered = useMemo(
    () => (hideDrafts ? hideDraftNodes(tree?.children ?? []) : (tree?.children ?? [])),
    [hideDrafts, tree],
  );
  /** 开关的渲染条件恒按**未过滤**的树算——否则开一次就再也关不回来了。 */
  const draftsInView = useMemo(() => containsDraft(tree?.children ?? []), [tree]);
  /** 新建 / 拖回根部 / 背景右键的落点：**真实 Vault 根**，绝不裁包裹层、绝不写死目录名。 */
  const rootTarget = "";
  const editInputRef = useRef<HTMLInputElement | null>(null);
  /** 屏幕上真的有那一行的路径，按渲染顺序——多选连选与方向键都按它取区间。 */
  const visibleRows = useMemo(
    () => visibleRowPaths(rendered, expandedDirs, (children) => sortTreeChildren(children, sortMode)),
    [rendered, expandedDirs, sortMode],
  );
  const selection = useNoteSelection({
    current: selectedPath,
    visible: visibleRows,
    // Delete 键 = 顶栏那颗删除按钮（同一条通道、同一张确认卡）。
    onDelete: () => void deleteSelection(),
    onToggleExpand: (path, expand) => {
      const node = findTreeNode(rendered, path);
      if (node?.is_dir) {
        setExpandedDirs((current) => {
          const next = new Set(current);
          if (expand) {
            next.add(path);
          } else {
            next.delete(path);
          }
          return next;
        });
      }
    },
  });
  /** 树头件数：此刻可见的文件数（跟着「隐藏待整理」走——三个数字里有一个不跟着开关走，
      用户会以为过滤没生效）。 */
  const noteCount = useMemo(() => countEntries(rendered), [rendered]);
  const contextMenu = useContextMenu();
  const noteMenu = useNoteMenu();

  // 打开输入框时聚焦并全选——**每次编辑只做一次**。依赖是"这是哪一次编辑"（kind + path），
  // 草稿不在其中：它在这里出现过，代价是每敲一个字符就把整串重新选上（见 `EditingState`）。
  const editingKey = editing ? `${editing.kind}:${editing.path}` : null;
  useEffect(() => {
    if (editingKey && editInputRef.current) {
      editInputRef.current.focus();
      editInputRef.current.select();
    }
  }, [editingKey]);

  // 「在全部笔记中显示」要真的**看得见**（PRD-07-006）：速记 / 搜索 / 标签 / 资料任一
  // 查询投影把真实 path 交过来时，它的祖先目录可能全是收起的——只把那一行标成选中，
  // 屏幕上什么都不会发生（那一行压根没被渲染）。故选中路径一变就摊开它的整条祖先链。
  //
  // **只在选中路径变化时做一次**：用户随后自己把哪一层收起来是他的事，`selectedPath`
  // 没变就不会再展开一次——否则收起某一层会被下一次渲染立刻撤销。
  useEffect(() => {
    if (!selectedPath) {
      return;
    }
    const ancestors = ancestorDirsOf(selectedPath);
    if (ancestors.length === 0) {
      return;
    }
    setExpandedDirs((current) => {
      if (ancestors.every((dir) => current.has(dir))) {
        return current;
      }
      const next = new Set(current);
      for (const dir of ancestors) {
        next.add(dir);
      }
      return next;
    });
  }, [selectedPath]);

  // 「有 N 篇待整理内容攒下了」那张体检卡的处置动作（PRD-06-055）：掀开「隐藏待整理」。
  //
  // **不再切什么模式**：树只有一棵真实的（ADR-087 ①），待整理内容一直在场，
  // 关掉过滤即可看见。除此之外什么都不做——整理与否是用户的判断（PRD-06-053）。
  // 从系统拖进来的文件（W5a，`lib/tree-file-drop.ts`）：复制进落点文件夹。收不收、叫什么名字
  // 全由 Rust 判，这里只展开那个文件夹、只进来一个就打开它、如实报结果。
  const treeRef = useRef<HTMLElement | null>(null);
  const importDroppedRef = useRef<(drop: TreeFileDrop) => Promise<void>>(async () => {});
  importDroppedRef.current = async ({ paths, folder }) => {
    const result = await commands.importFilesIntoFolder(paths, folder);
    if (result.status === "error") {
      onStatus(vaultErrorText(result.error));
      return;
    }
    const { imported, skipped } = result.data;
    if (imported.length > 0 && folder !== "") {
      setExpandedDirs((current) => new Set(current).add(folder));
    }
    if (imported.length === 1) {
      onSelect(imported[0]);
    }
    if (skipped.length === 0) {
      onStatus(t("tree.imported", { count: imported.length }));
      return;
    }
    const first = { name: skipped[0].name, reason: t(`tree.importSkipped.${skipped[0].reason}`) };
    onStatus(
      imported.length === 0
        ? t("tree.importedNone", { count: skipped.length, ...first })
        : t("tree.importedPartly", { count: imported.length, skipped: skipped.length, ...first }),
    );
  };
  useEffect(() => {
    const element = treeRef.current;
    if (!element) return;
    const onDrop = (event: Event) => {
      void importDroppedRef.current((event as CustomEvent<TreeFileDrop>).detail);
    };
    element.addEventListener(TREE_FILE_DROP_EVENT, onDrop);
    return () => element.removeEventListener(TREE_FILE_DROP_EVENT, onDrop);
  }, []);

  useEffect(() => {
    const onShowDrafts = () => setHideDrafts(false);
    window.addEventListener(SHOW_DRAFTS_EVENT, onShowDrafts);
    return () => window.removeEventListener(SHOW_DRAFTS_EVENT, onShowDrafts);
  }, []);

  function reportError() {
    onStatus(t("status.error"));
  }

  function toggleDir(path: string) {
    setExpandedDirs((current) => {
      const next = new Set(current);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
      }
      return next;
    });
  }

  function expandDir(path: string) {
    if (!path) {
      return;
    }
    setExpandedDirs((current) => {
      if (current.has(path)) {
        return current;
      }
      const next = new Set(current);
      next.add(path);
      return next;
    });
  }

  /** 移动/改名后同步展开集与选中路径的旧前缀（并通知外层重映射打开中的标签）。 */
  function remapPaths(from: string, to: string) {
    onMoved?.(from, to);
    setExpandedDirs((current) => {
      const next = new Set<string>();
      for (const dir of current) {
        if (dir === from || dir.startsWith(`${from}/`)) {
          next.add(to + dir.slice(from.length));
        } else {
          next.add(dir);
        }
      }
      return next;
    });
    if (selectedPath && (selectedPath === from || selectedPath.startsWith(`${from}/`))) {
      onSelect(to + selectedPath.slice(from.length));
    }
  }

  function startCreate(parent: string, kind: "create-note" | "create-folder") {
    expandDir(parent);
    setEditing({ kind, path: parent, isDir: kind === "create-folder" });
    setEditDraft("");
  }

  function startRename(path: string) {
    setEditing({ kind: "rename", path, isDir: findTreeNode(rendered, path)?.is_dir ?? false });
    // 草稿 = 用户在这一行上看到的那个名字（`.md` 已去掉，其它扩展名原样）——与行显示
    // 同一个函数。扩展名要不要保留是 Rust 的事（`vault_rename`），前端不猜。
    setEditDraft(noteStem(path));
  }

  /** 顶栏删除：作用于**选中的这一批**（单选时就是那一个）。 */
  async function deleteSelection() {
    const paths = selection.ordered;
    if (paths.length === 0) {
      return;
    }
    const only = paths.length === 1 ? findTreeNode(rendered, paths[0]) : null;
    await deleteNotesWithConfirm({
      paths,
      confirm:
        only === null
          ? {
              title: tCommon("menu.deleteSelectedTitle", { count: paths.length }),
              description: deleteConsequence,
              confirmLabel: tCommon("action.delete"),
            }
          : {
              title: only.is_dir
                ? t("main.deleteFolderTitle", { path: only.path })
                : t("main.deleteNoteTitle"),
              description: deleteConsequence,
              confirmLabel: tCommon("action.delete"),
            },
      ask,
      onDeleted: (deleted) => deleted.forEach(afterDeleted),
      onError: reportError,
    });
  }

  /** 删除成功后的收尾（关标签、清选中）——工具栏与右键菜单共用。 */
  function afterDeleted(path: string) {
    onDeleted?.(path);
    if (selectedPath && (selectedPath === path || selectedPath.startsWith(`${path}/`))) {
      onSelect(null);
    }
  }

  /**
   * 节点右键菜单：文件与文件夹共用同一套（`note-menu.tsx`），差别只在能力开关——
   * 文件夹没有正文可导出、但可以在其下新建；文件可以用各种方式打开。
   *
   * **直接吃 `NoteTreeDto`**：这里此前收一个手抄了五个字段的内联结构（且每个都是
   * `?: boolean` 再 `=== true` 兜一遍）。那是一层没有职责的适配：Rust 每多下发一位
   * 能力，就要记得在这个中间形状里再补一次，忘了不会有任何东西变红——菜单只是悄悄
   * 少一项。能力位的事实源只有 DTO 一个，就直接用它。
   */
  /**
   * 「从搜索与图谱中排除」（PRD-06-062，V4a）：文件夹与单篇都可以。被上级规则覆盖时如实说明
   * 规则挂在哪一层，不给一个点了也不生效的开关（与 AI 关系标签范围同一口径）。
   */
  function excludeEntry(node: NoteTreeDto): ContextMenuEntry {
    const path = node.path;
    const rule = node.excluded_by;
    if (rule !== null && rule !== path) {
      return {
        kind: "action",
        id: "excludedByAncestor",
        label: t("tree.excludedBy", { folder: rule }),
        disabled: true,
        onSelect: () => {},
      };
    }
    return {
      kind: "action",
      id: "toggleExcluded",
      label: t("tree.exclude"),
      checked: rule === path,
      onSelect: () => {
        // 改完 Rust 广播规则文件的变更，树随之重取、淡显与勾选态跟上。
        void commands.setExcludedPath(path, rule !== path).then((result) => {
          if (result.status === "error") {
            onStatus(vaultErrorText(result.error));
          }
        });
      },
    };
  }

  /** 「添加书签 / 移除书签」（PRD-06-006，V5b）：文件与文件夹都可以。 */
  function bookmarkEntry(path: string): ContextMenuEntry {
    const bookmarked = bookmarks.isBookmarked(path);
    return {
      kind: "action",
      id: "toggleBookmark",
      label: bookmarked ? t("bookmarks.remove") : t("bookmarks.add"),
      icon: <BookmarkIcon size={13} />,
      onSelect: () => {
        void bookmarks.setBookmarked(path, !bookmarked).then((error) => {
          if (error !== null) {
            onStatus(vaultErrorText(error));
          }
        });
      },
    };
  }

  function openNodeMenu(event: ReactMouseEvent, node: NoteTreeDto) {
    // 右键先把选择落定：落在选中集里保持整批，落在集外则单选它（Finder 同口径）。
    // 作用对象**同一拍**就要拿到（`targetsFor`），不能等这次状态更新落地后再读。
    selection.contextClick(node.path);
    const targets = selection.targetsFor(node.path);
    const batch = targets.length > 1;
    contextMenu.open(
      event,
      noteMenu({
        path: node.path,
        paths: targets,
        // 「汇总成一篇」的资格由 Rust 逐行下发（`can_merge`，只挡聚合页；落盘前
        // `merge_notes` 还会按同一个函数再验一次）。多选才给——一条不是汇总。
        mergeable: batch && targets.every((p) => findTreeNode(rendered, p)?.can_merge === true),
        // 多选时判据同样按**整批**算：一批里混进一个文件夹或一篇正式笔记，就不该给一个
        // 注定被逐条拒绝的动作（单选时判据仍是右键那一行自己）。
        // 「沉淀为知识」（PRD-06-053）：记录态笔记才给。判据是 Rust 下发的已判定布尔，
        // 前端不认识 `luna.stage` 的任何取值（ADR-003 / `check-note-stage-single-source.py`）。
        settleable: node.draft,
        // 「生成知识笔记」（PRD-06-044/054）：判据是 Rust 下发的 `distillable`
        // （= 资料 ∪ 记录，`NoteStage::allows_distill`）。
        // **不拿 `draft` 当它**：那样一篇录音在录音清单里有这一项、在文件树里没有
        // （2026-09-02 真机上报「批量生成知识笔记没有入口」——选中的正是三篇录音）。
        // 与「沉淀为知识」刻意分开：那是记录态原地去掉一个键，这是跑模型另出一篇，
        // 记录态两项**同时**给（PRD-06-054），互不取代。
        distillable: batch
          ? targets.every((p) => findTreeNode(rendered, p)?.distillable === true)
          : node.distillable,
        // 「标记为记录」（PRD-06-058）：沉淀的反向动作。判据同样由 Rust 判完下发
        // （知识态 ∧ 非主题页 ∧ 非知识地图 ∧ 是 `.md`），前端不自己拼这四个条件。
        markableAsDraft: node.markable_as_draft,
        onSettled: (to) => {
          if (to !== node.path) {
            remapPaths(node.path, to);
          }
        },
        leading: node.is_dir
          ? [
              {
                kind: "action",
                id: "newNote",
                label: t("tree.newNote"),
                icon: <PlusIcon size={13} />,
                onSelect: () => startCreate(node.path, "create-note"),
              },
              {
                kind: "action",
                id: "newFolder",
                label: t("tree.newFolder"),
                icon: <FolderPlusIcon size={13} />,
                onSelect: () => startCreate(node.path, "create-folder"),
              },
              separator("graphTags"),
              // AI 关系标签范围（PRD-06-035c）：停用/定向重扫，与功能清单同一构建器。
              graphScopeSubmenu(node.path, tagExclusions, t, onStatus),
              excludeEntry(node),
              bookmarkEntry(node.path),
              // 批量整理成知识笔记（PRD-06-044/051）：与上一条同类（对此文件夹的 AI
              // 批量动作）。真机反馈"未看到批量入口"——用户最自然的路径正是在文件树里
              // 右键资料区的 Recordings/，此前入口只挂在四条功能清单的文件夹上。
              //
              // **不再按目录分流**（ADR-087 / PRD-06-047）：此前速记目录下给的是
              // 「多条合成一篇」、其余给「逐篇各出一篇纪要」——而速记会被移动、普通笔记
              // 也会被标记为待整理，目录从来不等于成熟度。「多条 → 一篇」的入口已迁到
              // 「速记」查询投影里按真实 paths 选择（PRD-06-047）。
              distillFolderAction(node.path, t),
            ]
          : [bookmarkEntry(node.path), excludeEntry(node), ...syncEntries(node)],
        openInNewTab: node.is_dir ? undefined : () => onSelect(node.path, { newTab: true }),
        // 创建副本 / 用默认应用打开（V5a）：只对文件。副本建好就打开它（Finder / Obsidian 同样
        // 把焦点交给新的那一份），按常驻打开——这是明确意图。
        duplicate: node.is_dir ? undefined : { onDuplicated: (copy) => onSelect(copy, { keep: true }) },
        openExternally: !node.is_dir,
        openInSplit: node.is_dir || !onOpenInSplit ? undefined : () => onOpenInSplit(node.path),
        openInWindow: node.is_dir || !onOpenInWindow ? undefined : () => onOpenInWindow(node.path),
        // 「移动到 ▸」（PRD-07-062 的入口，这里的候选是**整棵真实的树**）。
        // **拖拽有了也仍要给它**：把四条选中的笔记拖到一个需要先滚动、先展开的文件夹
        // 里，鼠标要一直按着穿过半屏——那是能做到但没人愿意做的操作（2026-09-02 真机
        // 上报"移动到 未看到入口"）。非法落点（选中的文件夹自己与它的子孙）直接不列，
        // 已经全在那儿的由菜单置灰。
        moveTo: {
          choices: dirChoices(rendered, (children) => sortTreeChildren(children, sortMode)).filter(
            (choice) =>
              targets.every(
                (path) => choice.path !== path && !choice.path.startsWith(`${path}/`),
              ),
          ),
          rootLabel: tCommon("menu.folderRoot"),
          onMove: (targetDir, paths) => void moveEntries(paths, targetDir),
        },
        // 文件树有行内输入态，故把输入面留在原地；能力本身归菜单（别处弹输入卡）。
        rename: { inline: () => startRename(node.path) },
        exportText: !node.is_dir,
        remove: {
          confirm: {
            title: node.is_dir
              ? t("main.deleteFolderTitle", { path: node.path })
              : t("main.deleteNoteTitle"),
            description: deleteConsequence,
            confirmLabel: tCommon("action.delete"),
          },
          onDeleted: (deleted) => deleted.forEach(afterDeleted),
        },
        onStatus: onStatus,
      }),
    );
  }

  /** 官方同步的本机副本（ADR-156）：只在云端的给「下载到本机」，同步中的录音给「释放本机空间」。 */
  function syncEntries(node: NoteTreeDto): ContextMenuEntry[] {
    const name = node.path.split("/").pop() ?? node.path;
    if (node.sync_cloud_only) {
      return [
        {
          kind: "action",
          id: "syncDownload",
          label: t("cloud.sync_download"),
          icon: <DownloadIcon size={13} />,
          onSelect: () => void cloudOnly.download(node.path, name),
        },
      ];
    }
    if (node.media && cloudOnly.synced) {
      return [
        {
          kind: "action",
          id: "syncEvict",
          label: t("cloud.sync_evict"),
          icon: <CloudUploadIcon size={13} />,
          onSelect: () => void cloudOnly.evict(node.path, name),
        },
      ];
    }
    return [];
  }

  /** 空白处（= Vault 根）：只给"在这里新建 / 整棵树的操作"，不给针对某一节点的动作。 */
  function openRootMenu(event: ReactMouseEvent) {
    contextMenu.open(
      event,
      buildMenu([
        {
          kind: "action",
          id: "newNote",
          label: t("tree.newNote"),
          icon: <PlusIcon size={13} />,
          onSelect: () => startCreate(rootTarget, "create-note"),
        },
        {
          kind: "action",
          id: "newFolder",
          label: t("tree.newFolder"),
          icon: <FolderPlusIcon size={13} />,
          onSelect: () => startCreate(rootTarget, "create-folder"),
        },
        {
          kind: "action",
          id: "newFromTemplate",
          label: t("template.title"),
          icon: <FileIcon size={13} />,
          onSelect: onOpenTemplatePicker,
        },
        separator("tree"),
        {
          kind: "action",
          id: "collapseAll",
          label: t("main.collapseAll"),
          icon: <FoldIcon size={13} />,
          onSelect: () => setExpandedDirs(new Set()),
        },
        {
          kind: "action",
          id: "revealRoot",
          label: tCommon("menu.revealInFileManager"),
          icon: <FolderIcon size={13} />,
          onSelect: () => {
            void commands.revealVaultPath(rootTarget).then((result) => {
              if (result.status === "error") {
                reportError();
              }
            });
          },
        },
      ]),
      // 背景菜单作用于整棵树，没有"某一行"可高亮。
      { highlight: null },
    );
  }

  /** 排序菜单：三档口径，勾选态即当前口径（与回收站同一套菜单语言，PRD-07-059）。 */
  function openSortMenu(event: ReactMouseEvent) {
    contextMenu.open(
      event,
      buildMenu(
        TREE_SORT_MODES.map((option) => ({
          kind: "action" as const,
          id: `sort-${option}`,
          label: t(`tree.sort.${option}`),
          checked: sortMode === option,
          onSelect: () => setSortMode(option),
        })),
      ),
      { highlight: null },
    );
  }

  async function commitEditing() {
    const current = editing;
    if (!current) {
      return;
    }
    const name = editDraft.trim();
    if (!name) {
      setEditing(null);
      return;
    }
    setEditing(null);

    if (current.kind === "create-folder") {
      const path = current.path ? `${current.path}/${name}` : name;
      const result = await commands.vaultCreateDir(path);
      if (result.status === "error") {
        reportError();
        return;
      }
      expandDir(path);
      return;
    }

    if (current.kind === "create-note") {
      const stem = name.replace(/\.md$/u, "");
      const path = current.path ? `${current.path}/${stem}.md` : `${stem}.md`;
      const result = await commands.vaultCreate(path, "note", "");
      if (result.status === "error") {
        reportError();
        return;
      }
      onSelect(result.data.path);
      return;
    }

    // 改名：笔记 / 资料文件 / 文件夹**同一条命令**（`vault_rename`）。同目录拼接、
    // 扩展名保留、非法字符拒绝全在 Rust——前端交上去的只是用户看到的那个名字。
    const result = await afterEditorsSaved(
      () => commands.vaultRename(current.path, name),
      () => ({ status: "error" as const, error: tCommon("saveBlocksMove") }),
    );
    if (result.status === "error") {
      // Rust 的拒绝有明确原因（名称为空 / 含分隔符 / 落点已存在），原样回流；
      // 报一句泛泛的"操作失败"等于把它已经算好的解释扔掉。
      onStatus(vaultErrorText(result.error));
      return;
    }
    remapPaths(current.path, result.data.path);
    announceRelocation(result.data.receipt);
  }

  /** 把这一批拖进某个目录：落点路径归 Rust（`vault_move_into`），前端不拼。 */
  async function moveEntries(sourcePaths: readonly string[], targetDir: string) {
    if (sourcePaths.length === 0) {
      return;
    }
    const result = await afterEditorsSaved(
      () => commands.vaultMoveInto([...sourcePaths], targetDir),
      () => ({ status: "error" as const, error: tCommon("saveBlocksMove") }),
    );
    if (result.status === "error") {
      onStatus(vaultErrorText(result.error));
      return;
    }
    expandDir(targetDir);
    // 重映射按 Rust **实际搬走**的那几条走（已在落点目录里的跳过、失败的不算）——前端
    // 再拼一遍落点路径就是同一条规则的第二份实现。
    for (const moved of result.data.moved) {
      remapPaths(moved.from, moved.to);
    }
    announceRelocation(result.data.receipt);
    // 没搬成的必须说出来：真机上四个拖成三个，界面只字未提，用户只能靠重启后自己
    // 对着文件夹数（2026-09-02）。理由由 Rust 逐项给，这里报第一条 + 条数。
    if (result.data.failed.length > 0) {
      onStatus(
        tCommon("menu.batchFailed", {
          count: result.data.failed.length,
          reason: vaultErrorText(result.data.failed[0].error),
        }),
      );
    }
  }

  // 树内移动走**指针事件**而非 HTML5 DnD——后者与 Tauri 的 OS 文件拖放互斥
  // （见 `lib/pointer-drag.ts` 的取舍说明，PRD-05-001）。
  const drag = usePointerDrag<readonly string[]>((sourcePaths, targetDir) => {
    // **松手与高亮同一判据**：不合法时高亮本来就没亮，此处再放行等于让用户看着
    // 一个"放不进去"的落点却真的把命令发出去。整批仍由 Rust 权威复验（前端这一份是
    // 画不画高亮的判断，不是落盘判据）。
    if (!canDropSelection(sourcePaths, targetDir)) {
      return;
    }
    void moveEntries(sourcePaths, targetDir);
  }, (sourcePaths, point) => {
    // 松手在编辑器正文上（不在任何文件夹上）：插入指向它们的链接（对标方案 B4）。
    dropOnEditor("vault", sourcePaths, point.x, point.y);
  });
  // 拖过正文时给它描边（与系统文件拖入同一个放置态），离开或松手即撤。
  const overEditor = drag.payload !== null && drag.dropId === null;
  useEffect(() => {
    markEditorDropTarget(overEditor ? editorSurfaceAt(drag.position.x, drag.position.y) : null);
  }, [overEditor, drag.position]);
  // 落点高亮：拖拽中且这一批**真的能放进去**才亮（不能就不给"能放"的错觉）。
  const dropTarget =
    drag.payload !== null && drag.dropId !== null && canDropSelection(drag.payload, drag.dropId)
      ? drag.dropId
      : null;

  /**
   * 行内输入框——**新建与改名共用这一个**（`label` 只换可访问名）。
   *
   * 它此前是两份逐字重复的 `<input>`（新建一份、改名一份），四个事件处理器各写了两遍。
   * 那正是"全选 bug 只在改名那一份里被看见、修的时候容易只修一处"的土壤。
   */
  function renderEditInput(label: string) {
    return (
      <input
        ref={editInputRef}
        type="text"
        className="luna-input luna-input--sm luna-note-tree__edit-input"
        value={editDraft}
        aria-label={label}
        placeholder={t("tree.namePlaceholder")}
        onChange={(event) => setEditDraft(event.target.value)}
        onKeyDown={(event) => {
          if (isImeKeyEvent(event)) return;
          if (event.key === "Enter") {
            event.preventDefault();
            void commitEditing();
          } else if (event.key === "Escape") {
            setEditing(null);
          }
        }}
        onBlur={() => void commitEditing()}
      />
    );
  }

  function renderEditRow(depthClass = "") {
    if (!editing) {
      return null;
    }
    return (
      <li className={`luna-note-tree__item ${depthClass}`.trim()}>
        <div className="luna-note-tree__edit-row">
          <span className="luna-note-tree__chevron-placeholder" />
          {editing.isDir ? (
            <FolderIcon size={14} className="luna-note-tree__icon luna-note-tree__icon-dir" />
          ) : (
            <FileIcon size={14} className="luna-note-tree__icon luna-note-tree__icon-file" />
          )}
          {renderEditInput(t("tree.namePlaceholder"))}
        </div>
      </li>
    );
  }

  function renderTreeItems(items: readonly NoteTreeDto[], parentPath: string): ReactNode {
    const sorted = sortTreeChildren(items, sortMode);
    return (
      <>
        {editing && editing.kind !== "rename" && editing.path === parentPath && renderEditRow()}
        {sorted.map((item) => {
          const expanded = expandedDirs.has(item.path);
          const displayName = noteStem(item.path);
          const isRenaming = editing?.kind === "rename" && editing.path === item.path;
          return (
            /* `role="none"`：行的语义在按钮上（`treeitem`），`<li>` 只是排版容器——
               留着它的 listitem 语义会在 `role="tree"` 里多插一层读屏念得出来的空壳。 */
            <li key={item.path} className="luna-note-tree__item" role="none">
              {isRenaming ? (
                renderEditRowInline(item)
              ) : (
                <button
                  type="button"
                  className="luna-note-tree__item-btn"
                  // 树是**可多选的树**：`treeitem` + `aria-selected` 才有"这一批被选中"
                  // 这句话可说（`aria-current` 答的是另一个问题——哪一条是打开着的，
                  // 裁决 D5）。`aria-selected` 由 `rowProps` 贴；方向键 / Shift 连选 /
                  // ← → 展开收起由 `useNoteSelection` 在容器上兑现，声明的角色不留空头支票。
                  role="treeitem"
                  {...selection.rowProps(item.path)}
                  data-dir={item.is_dir}
                  // 记录态淡显（PRD-06-052）：判定由 Rust 下发，前端只挂标记、样式归 CSS。
                  data-draft={item.draft || undefined}
                  // iCloud 未下载（TDD-06 §4.1 ②）：判定由 Rust 下发，淡显同记录态口径。
                  data-cloud={item.cloud_placeholder || item.sync_cloud_only || undefined}
                  // 排除路径（V4a）：判定由 Rust 下发，淡显同记录态口径。
                  data-excluded={item.excluded_by !== null || undefined}
                  // 隐藏项（V4b，「显示隐藏文件」打开时才在树里）：判定由 Rust 下发，同口径淡显。
                  data-hidden={item.hidden || undefined}
                  data-drop-target={dropTarget === item.path || undefined}
                  aria-current={selectedPath === item.path || undefined}
                  aria-expanded={item.is_dir ? expanded : undefined}
                  title={displayName}
                  data-drop-id={item.is_dir ? item.path : parentPath}
                  onPointerDown={(event) => {
                    // 拖的是**这一批**：拖起选中集里的任一行就整批一起走；拖一行集外的
                    // 就只拖它（右键同口径）。
                    drag.begin(event, selection.contains(item.path) ? selection.ordered : [item.path]);
                  }}
                  onContextMenu={(event) => openNodeMenu(event, item)}
                  onClick={(event) => {
                    // 先落选择：Cmd/Ctrl = 加选、Shift = 连选，两者都**不打开**——每加选
                    // 一条就把工作面切过去，等于让用户没法一次选中三条（PRD-06-059）。
                    if (!selection.click(item.path, event)) {
                      return;
                    }
                    if (item.is_dir) {
                      toggleDir(item.path);
                      onSelect(item.path);
                      return;
                    }
                    // 还在 iCloud 上：打不开，先请系统下载；下载回来这一行变回普通笔记。
                    if (item.cloud_placeholder) {
                      void commands.requestCloudDownload(item.path).then((result) => {
                        onStatus(
                          result.status === "ok"
                            ? t("cloud.downloading", { name: displayName })
                            : vaultErrorText(result.error),
                        );
                      });
                      return;
                    }
                    // 只在云端（官方同步按需下载，ADR-156）：先下到本机，下好了再打开。
                    if (item.sync_cloud_only) {
                      void cloudOnly.download(item.path, displayName).then((ready) => {
                        if (ready) onSelect(item.path);
                      });
                      return;
                    }
                    onSelect(item.path);
                  }}
                  onDoubleClick={() => {
                    // 双击 = 要留着它：预览标签转常驻（PRD-07-035）；目录双击仍只是展开收起。
                    if (!item.is_dir && !item.cloud_placeholder && !item.sync_cloud_only) {
                      onSelect(item.path, { keep: true });
                    }
                  }}
                  onAuxClick={(event) => {
                    // 中键 = 新标签打开（浏览器/IDE 惯例；仅文件）。
                    if (event.button === 1 && !item.is_dir) {
                      event.preventDefault();
                      onSelect(item.path, { newTab: true });
                    }
                  }}
                >
                  {item.is_dir ? (
                    <ChevronRightIcon
                      className="luna-note-tree__chevron"
                      data-expanded={expanded}
                      size={12}
                    />
                  ) : (
                    <span className="luna-note-tree__chevron-placeholder" />
                  )}
                  {item.is_dir ? (
                    <FolderIcon
                      size={14}
                      className="luna-note-tree__icon luna-note-tree__icon-dir"
                    />
                  ) : (
                    <FileNodeIcon
                      path={item.path}
                      className="luna-note-tree__icon luna-note-tree__icon-file"
                    />
                  )}
                  <span className="luna-note-tree__item-name">{displayName}</span>
                  {item.cloud_placeholder && (
                    <span className="luna-source-badge" title={t("cloud.hint")}>
                      {t("cloud.badge")}
                    </span>
                  )}
                  {item.sync_cloud_only && (
                    <span className="luna-source-badge" title={t("cloud.sync_hint")}>
                      <CloudOnlyBadgeText progress={cloudOnly.progress} path={item.path} />
                    </span>
                  )}
                  {conflictCopies.has(item.path) && (
                    <span className="luna-conflict-badge" title={t("editor:conflictCopy.badge")}>
                      {t("editor:conflictCopy.badge")}
                    </span>
                  )}
                  {/* 类型徽标（PRD-06-048①）：来源层的三种笔记各写自己的类型名——录音 /
                      转写 / 对话，而不是一个笼统的「资料」（「资料」不作界面用词，PRD-07 §6）。
                      判据与词都来自 Rust 下发（`is_source_material` / `note_type`），前端不
                      认识"哪几种类型算来源"（ADR-003）。
                      **身份必须长在对象上**：它此前靠"树被切成知识段 / 来源段"表达，
                      而那是一套第二归属事实源（ADR-087 ①）。段没了，标记就得回到行上，
                      否则真实树里来源与知识长得一模一样。 */}
                  {item.is_source_material && item.note_type && (
                    <span className="luna-source-badge" title={t("panel.noteInfoSourceHint")}>
                      {t(noteTypeLabelKey(item.note_type))}
                    </span>
                  )}
                  {/* 「记录」徽标（PRD-06-052）：与类型徽标**刻意不同形**——类型是
                      实线描边的中性胶囊，记录是虚线描边（未定稿的通用视觉语言）。两枚
                      同形只差一个字，在 13px 的侧栏里等于没区分。
                      两枚不会同时出现：来源轴优先，来源层笔记上的 `stage` 键被 Rust 忽略。 */}
                  {item.draft && (
                    <span className="luna-draft-badge" title={t("tree.draftHint")}>
                      {t("tree.draft")}
                    </span>
                  )}
                </button>
              )}
              {item.is_dir &&
              (expanded || (editing && editing.kind !== "rename" && editing.path === item.path)) ? (
                <ul className="luna-note-tree__items" role="group">
                  {renderTreeItems(item.children, item.path)}
                </ul>
              ) : null}
            </li>
          );
        })}
      </>
    );
  }

  function renderEditRowInline(item: NoteTreeDto) {
    return (
      <div className="luna-note-tree__edit-row">
        {item.is_dir ? (
          <ChevronRightIcon
            className="luna-note-tree__chevron"
            data-expanded={expandedDirs.has(item.path)}
            size={12}
          />
        ) : (
          <span className="luna-note-tree__chevron-placeholder" />
        )}
        {item.is_dir ? (
          <FolderIcon size={14} className="luna-note-tree__icon luna-note-tree__icon-dir" />
        ) : (
          <FileIcon size={14} className="luna-note-tree__icon luna-note-tree__icon-file" />
        )}
        {renderEditInput(t("tree.rename"))}
      </div>
    );
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
          ? baseNameOf(drag.payload[0])
          : t("tree.dragCount", { count: drag.payload.length })}
      </div>
    );

  return (
    <section
      ref={treeRef}
      className="luna-note-tree"
      data-drop-target={dropTarget === rootTarget || undefined}
      onContextMenu={openRootMenu}
      data-drop-id={rootTarget}
    >
      {/* 名字由主侧栏的二级分区导航给（ADR-087 ①）——面板再写一遍就是同一事实在同一屏
          出现两次（PRD-07-065）。这里只留「有多少」与「能做什么」。 */}
      <header className="luna-note-tree__header luna-sidebar-header">
        <div className="luna-sidebar-header__title">
          <span className="luna-count-badge">{noteCount}</span>
        </div>
        {/* 图标按钮一律 `title` + `aria-label` 双写：`title` 是鼠标的提示，读屏取的是
            可访问名——此前六颗全靠 `title`，其中「从模板新建」还把字母 T 套在
            `aria-hidden` 里，于是那颗按钮在读屏下**根本没有名字**。 */}
        <div className="luna-note-tree__actions">
          <IconButton
            label={t("main.createNote")}
            icon={<PlusIcon size={14} />}
            onClick={onCreateNote}
          />
          <IconButton
            label={t("tree.newFolder")}
            icon={<FolderPlusIcon size={14} />}
            onClick={() => startCreate(rootTarget, "create-folder")}
          />
          <IconButton
            label={t("template.title")}
            icon={<TemplateIcon size={14} />}
            onClick={onOpenTemplatePicker}
          />
          <IconButton
            label={t("tree.sortLabel")}
            icon={<SortIcon size={14} />}
            aria-haspopup="menu"
            onClick={openSortMenu}
          />
          {/* 「隐藏记录」（PRD-06-052）：只在这一片真有记录态笔记时出现——一个没有可
              隐藏对象的开关只是占位的噪音（同段头「只在两段同时在场时才贴标签」）。
              `aria-pressed` 表达按下态（仓库裁决 D5），`label` 随态改写以便读屏念出
              此刻按下去会发生什么。 */}
          {draftsInView && (
            <IconButton
              label={hideDrafts ? t("tree.showDrafts") : t("tree.hideDrafts")}
              icon={<FilterIcon size={14} />}
              aria-pressed={hideDrafts}
              onClick={() => setHideDrafts((value) => !value)}
            />
          )}
          <IconButton
            label={t("main.collapseAll")}
            icon={<FoldIcon size={14} />}
            onClick={() => setExpandedDirs(new Set())}
          />
          {/* 删除作用于**选中的那个节点**，故它是不是目录只能问树里的那个节点。此前用
              「路径不以 .md 结尾就当目录」猜——选中一张图片附件（`assets/x.png`）会弹出
              删除文件夹的确认文案，而用户要删的是一个文件。选中项已不在树里（刚被外部
              删掉、或还没加载）时按钮不可用：拿不到事实就不动手。 */}
          <IconButton
            tone="danger"
            disabled={selection.ordered.length === 0}
            // 多选时按钮说的是**这一批**：写着「删除笔记」却删掉五个对象，是在动手前
            // 少说了一句最要紧的话。
            label={
              selection.multiple
                ? tCommon("menu.deleteSelected", { count: selection.ordered.length })
                : t("main.deleteNote")
            }
            icon={<TrashIcon size={14} />}
            onClick={() => void deleteSelection()}
          />
        </div>
      </header>
      {/* 书签（V5b）：在树的上方，点笔记与树里单击同一种打开方式；点文件夹在树里定位并展开。 */}
      <BookmarksSection
        bookmarks={bookmarks}
        onStatus={onStatus}
        onOpen={(bookmark) => {
          onSelect(bookmark.path);
          if (bookmark.is_dir) {
            expandDir(bookmark.path);
          }
        }}
      />
      {tree === null || rendered.length === 0 ? (
        <>
          {editing && editing.kind !== "rename" && editing.path === rootTarget ? (
            <ul className="luna-note-tree__items">{renderEditRow()}</ul>
          ) : tree === null ? (
            /* 尚在加载 ≠ 库是空的：两者此前共用「暂无 Markdown 笔记」，于是每次打开
               应用的第一帧都在说一句当时还不知道真假的话。 */
            <p className="luna-empty">{tCommon("status.loading")}</p>
          ) : hideDrafts ? (
            /* **被过滤空 ≠ 库是空的**（PRD-07 §3.4 的四类空态）：开着「隐藏待整理」
               时说「暂无 Markdown 笔记」并递上「新建笔记」，是在用户明明有笔记的时候
               告诉他一件假事，还把出路指向了错误的动作——他要的是把过滤关掉。 */
            <EmptyState
              icon={<FilterIcon size={24} />}
              title={t("tree.emptyFiltered")}
              description={t("tree.emptyFilteredHint")}
              action={
                <button
                  type="button"
                  className="luna-btn luna-btn--primary"
                  onClick={() => setHideDrafts(false)}
                >
                  {t("tree.showDrafts")}
                </button>
              }
            />
          ) : (
            <EmptyState
              icon={<NotebookIcon size={24} />}
              title={t("main.emptyTree")}
              description={t("main.emptyTreeHint")}
              action={
                // 空库只有一个明显的新建入口；模板是次动作（2026-10-08 W05 / C10）。
                <>
                  <button type="button" className="luna-btn luna-btn--primary" onClick={onCreateNote}>
                    {t("tree.newNote")}
                  </button>
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary"
                    onClick={onOpenTemplatePicker}
                  >
                    {t("template.title")}
                  </button>
                </>
              }
            />
          )}
        </>
      ) : (
        /* **一棵真实的树，仅此一棵**（ADR-087 ① / PRD-06-048 ①）：不裁 `notes_dir`
           包裹层、不做「全部 / 知识 / 资料」三态、不生成任何虚拟根。分层此前在这里长成了
           一套第二归属事实源——同一个目录出现在两个区段、切一次段控就像换了一个库，而
           磁盘上什么都没变。身份改由**对象自己**表达（行内的资料 / 待整理 / 主题页徽标，
           判定全在 Rust），这才是「区分长在界面上」而不是「把树拆成三棵」。 */
        <ul
          className="luna-note-tree__items"
          role="tree"
          aria-multiselectable="true"
          aria-label={t("main.noteTree")}
          {...selection.containerProps}
        >
          {renderTreeItems(rendered, "")}
        </ul>
      )}
      {dragGhost}
    </section>
  );
}
