/**
 * 副库独立窗口（PRD-06-050 / ADR-025）：浏览 / 编辑 / 新建 / 重命名 + 只读图谱。
 *
 * 主窗口恒承载唯一活跃库（全局采集的无歧义落点），"边写 A 库边查 B 库"经本窗口满足。
 * 能力边界（v1.1，真机反馈修订）：树浏览 + 正文编辑保存 + 新建笔记 + 标题重命名 +
 * wiki 链接窗口内导航与创建 + **只读图谱**（标签即图谱节点）；移动 / 删除 / AI 一律
 * 回主窗口——AI 的一切写回都以主库为落点，这里任何按路径的 AI 动作都是跨库数据污染面。
 *
 * 树是**专用的浏览树**而非复用工作区 NoteTree：后者深度耦合工作区能力（拖拽移动 /
 * 右键菜单 / 蒸馏与 AI 标签入口 / 按主库设置分区），逐项禁用得到的是一棵浑身残缺
 * 提示的树。但**视觉语言与主树同源**——直接复用 `.luna-note-tree__*` 行样式与
 * `FileNodeIcon`，两窗观感一致（真机反馈「样式未对齐」的修法是共用类，不是抄一份）。
 * 命令面由 Rust 按窗口身份路由到本窗的库（`vaultTree`/`readNote`/`writeNote`/
 * `createNote`/`vaultCreate`/`vaultRename`/`vaultGraph`）。
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { IconButton } from "../../components/ui/IconButton";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useTranslation } from "react-i18next";
import { commands, type NoteTreeDto } from "../../ipc/bindings";
import { VaultEditor } from "../editor/VaultEditor";
import { GraphView } from "../graph/GraphView";
import { useGraphController } from "../graph/use-graph-controller";
import { FileNodeIcon } from "../workspace/NoteTree";
import {
  ArrowClockwiseIcon,
  ChevronRightIcon,
  FileTextIcon,
  FolderIcon,
  GraphIcon,
  HardDriveIcon,
  PencilIcon,
  PlusIcon,
} from "../../components/ui/Icon";
import { EmptyState } from "../../components/ui/EmptyState";
import { vaultErrorText } from "../../lib/vault-error";

function TreeNode({
  node,
  selected,
  expanded,
  onToggle,
  onOpen,
}: {
  node: NoteTreeDto;
  selected: string | null;
  expanded: Set<string>;
  onToggle: (path: string) => void;
  onOpen: (path: string) => void;
}) {
  const isOpen = expanded.has(node.path);
  const displayName = node.is_dir ? node.name : node.name.replace(/\.md$/u, "");
  return (
    <li className="luna-note-tree__item">
      <button
        type="button"
        className="luna-note-tree__item-btn"
        data-dir={node.is_dir}
        aria-current={selected === node.path || undefined}
        aria-expanded={node.is_dir ? isOpen : undefined}
        title={displayName}
        onClick={() => (node.is_dir ? onToggle(node.path) : onOpen(node.path))}
      >
        {node.is_dir ? (
          <ChevronRightIcon className="luna-note-tree__chevron" data-expanded={isOpen} size={12} />
        ) : (
          <span className="luna-note-tree__chevron-placeholder" />
        )}
        {node.is_dir ? (
          <FolderIcon size={14} className="luna-note-tree__icon luna-note-tree__icon-dir" />
        ) : (
          <FileNodeIcon
            path={node.path}
            className="luna-note-tree__icon luna-note-tree__icon-file"
          />
        )}
        <span className="luna-note-tree__item-name">{displayName}</span>
      </button>
      {node.is_dir && isOpen ? (
        <ul className="luna-note-tree__items">
          {node.children.map((child) => (
            <TreeNode
              key={child.path}
              node={child}
              selected={selected}
              expanded={expanded}
              onToggle={onToggle}
              onOpen={onOpen}
            />
          ))}
        </ul>
      ) : null}
    </li>
  );
}

export function SecondaryVaultWindow() {
  const { t } = useTranslation("workspace");
  const [vaultName, setVaultName] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const [tree, setTree] = useState<NoteTreeDto | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const [selected, setSelected] = useState<string | null>(null);
  const [view, setView] = useState<"notes" | "graph">("notes");
  const [autoFocusTitle, setAutoFocusTitle] = useState(false);
  const [status, setStatus] = useState("");
  const windowLabel = useMemo(() => getCurrentWindow().label, []);
  // 图谱数据以视图激活门控（与主窗同一策略：不打开就不构图）。
  const graph = useGraphController(view === "graph");

  const refreshTree = useCallback(async () => {
    const result = await commands.vaultTree();
    setTree(result.status === "ok" ? result.data : null);
  }, []);

  useEffect(() => {
    let disposed = false;
    void commands.vaultWindowInfo().then((result) => {
      if (disposed) {
        return;
      }
      if (result.status === "ok" && result.data) {
        setVaultName(result.data);
        void getCurrentWindow().setTitle(result.data);
        void refreshTree();
      }
      setReady(true);
    });
    return () => {
      disposed = true;
    };
  }, [refreshTree]);

  const createNote = useCallback(async () => {
    // 与主窗「新建」同一条命令（自动命名归 Rust）；命令已按窗口身份落本库。
    const result = await commands.createNote("note");
    if (result.status === "ok") {
      setSelected(result.data.path);
      setView("notes");
      setAutoFocusTitle(true);
      void refreshTree();
    } else {
      setStatus(vaultErrorText(result.error));
    }
  }, [refreshTree]);

  if (!ready) {
    return <div className="luna-vault-window" data-loading="true" />;
  }
  if (vaultName === null) {
    return (
      <div className="luna-vault-window luna-vault-window--empty">
        <p>{t("vault.windowGone")}</p>
      </div>
    );
  }

  return (
    <div className="luna-vault-window">
      <aside className="luna-vault-window__side">
        <header className="luna-vault-window__head">
          <HardDriveIcon size={14} />
          <strong className="luna-vault-window__name" title={vaultName}>
            {vaultName}
          </strong>
          <IconButton
            label={t("vault.windowNewNote")}
            icon={<PlusIcon size={13} />}
            onClick={() => void createNote()}
          />
          <IconButton
            label={t("vault.windowRefresh")}
            icon={<ArrowClockwiseIcon size={13} />}
            onClick={() => void refreshTree()}
          />
        </header>
        {/* 能力边界如实说在界面上（PRD-06-050）：收窄是刻意的，不是故障。 */}
        <p className="luna-vault-window__hint">{t("vault.windowHint")}</p>
        {/* 笔记 / 图谱 视图切换：复用共享胶囊开关原语（`.luna-pill-toggle`，
            共享控件；.luna-editor__mode-toggle 的按钮样式挂在编辑器页头作用域下，
            游离使用会退化成原生按钮——真机反馈「样式不统一」的病根之一）。 */}
        <div className="luna-vault-window__views" role="group" aria-label={vaultName}>
          <button
            type="button"
            className="luna-pill-toggle"
            aria-pressed={view === "notes"}
            onClick={() => setView("notes")}
          >
            <PencilIcon size={13} />
            {t("vault.windowNotes")}
          </button>
          <button
            type="button"
            className="luna-pill-toggle"
            aria-pressed={view === "graph"}
            onClick={() => setView("graph")}
          >
            <GraphIcon size={13} />
            {t("vault.windowGraph")}
          </button>
        </div>
        <nav className="luna-vault-window__tree" aria-label={vaultName}>
          <ul className="luna-note-tree__items">
            {tree?.children.map((node) => (
              <TreeNode
                key={node.path}
                node={node}
                selected={selected}
                expanded={expanded}
                onToggle={(path) =>
                  setExpanded((current) => {
                    const next = new Set(current);
                    if (next.has(path)) {
                      next.delete(path);
                    } else {
                      next.add(path);
                    }
                    return next;
                  })
                }
                onOpen={(path) => {
                  setSelected(path);
                  setView("notes");
                }}
              />
            ))}
          </ul>
        </nav>
      </aside>
      <main className="luna-vault-window__main">
        {view === "graph" ? (
          <GraphView
            controller={graph}
            focus={null}
            restricted
            onOpenNote={(path) => {
              setSelected(path);
              setView("notes");
            }}
            onOpenTag={() => {
              // 副库窗口没有标签视图：标签的关联就在眼前这张图上，不跳转。
            }}
            onStatus={setStatus}
          />
        ) : selected === null ? (
          <EmptyState
            icon={<FileTextIcon size={24} />}
            title={t("vault.windowEmptyTitle")}
            description={t("vault.windowEmptyBody")}
          />
        ) : (
          <VaultEditor
            path={selected}
            restricted
            vaultContext={windowLabel}
            autoFocusTitle={autoFocusTitle}
            onTitleFocused={() => setAutoFocusTitle(false)}
            onStatus={setStatus}
            onOpenPath={(path) => setSelected(path)}
            onSavedPath={(path) => {
              // 重命名 / 首存改名后跟住新路径并翻新树。
              setSelected(path);
              void refreshTree();
            }}
          />
        )}
        <span className="luna-vault-window__status" role="status">
          {status}
        </span>
      </main>
    </div>
  );
}
