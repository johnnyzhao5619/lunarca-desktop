/**
 * 侧栏（PRD-21-011 / 022 / 024 / 040 / 041 / 042）：标题与路径搜索（即时，WASM 里按名字比较键匹配）或正文搜索
 * （在浏览器里建的全文索引）、文件夹树（与桌面同一个排序函数）、「最近」（按修改时间）、标签与最近删除；「新建笔记 /
 * 新建文件夹」落在当前文件夹——最近点开的那个文件夹，或正在看的那一篇所在的文件夹，都没有就是库的根。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { sortTreeChildren } from "@lunarca/editor-core/tree-sort";
import type { FileSummary, TreeNode, WrittenFile } from "../../worker/protocol";
import type { WorkerClient } from "../../worker/worker-client";
import { FulltextGate } from "../library/FulltextGate";
import { FulltextResults } from "../library/FulltextResults";
import { RecentlyDeleted } from "../library/RecentlyDeleted";
import { TagPanel } from "../library/TagPanel";
import { useFulltext } from "../library/use-fulltext";

export interface SidebarProps {
  client: WorkerClient;
  tree: TreeNode[];
  recent: FileSummary[];
  selectedId: string | null;
  /** 打开的那一篇的路径：换篇时把它所在的文件夹一路展开（链接、搜索、最近打开的都看得到）。 */
  selectedPath: string | null;
  onSelect(file: FileSummary): void;
  /** 写入闸关着（只读）时不能新建。 */
  canCreate: boolean;
  onNewNote(dir: string): void;
  onNewFolder(dir: string): void;
  /** 从「最近删除」恢复了一个文件。 */
  onRestored(file: WrittenFile): void;
}

type Tab = "tree" | "recent" | "tags" | "trash";
const TABS: readonly Tab[] = ["tree", "recent", "tags", "trash"];

function summaryOf(node: TreeNode): FileSummary | null {
  return node.file_id === null
    ? null
    : { file_id: node.file_id, path: node.path, name: node.name, size: node.size ?? 0, mtime_ms: node.mtime_ms };
}

function TreeLevel({
  nodes,
  dir,
  depth,
  expanded,
  onToggle,
  selectedId,
  onSelect,
}: {
  nodes: TreeNode[];
  /** 这一层所在的文件夹（库根为空串）。 */
  dir: string;
  depth: number;
  expanded: ReadonlySet<string>;
  onToggle(path: string): void;
  selectedId: string | null;
  onSelect(file: FileSummary, dir: string): void;
}) {
  return (
    <ul className="luna-web-tree__level" role={depth === 0 ? "tree" : "group"}>
      {sortTreeChildren(nodes, "name").map((node) => {
        if (node.is_dir) {
          const open = expanded.has(node.path);
          return (
            <li key={node.path} role="treeitem" aria-expanded={open} aria-selected={false}>
              <button
                type="button"
                className="luna-web-tree__row luna-web-tree__row--dir"
                data-depth={Math.min(depth, 8)}
                onClick={() => onToggle(node.path)}
              >
                {node.name}
              </button>
              {open ? (
                <TreeLevel
                  nodes={node.children}
                  dir={node.path}
                  depth={depth + 1}
                  expanded={expanded}
                  onToggle={onToggle}
                  selectedId={selectedId}
                  onSelect={onSelect}
                />
              ) : null}
            </li>
          );
        }
        const file = summaryOf(node);
        return (
          <li key={node.path} role="treeitem" aria-selected={file?.file_id === selectedId}>
            <button
              type="button"
              className="luna-web-tree__row"
              data-depth={Math.min(depth, 8)}
              aria-current={file?.file_id === selectedId ? "page" : undefined}
              onClick={() => file && onSelect(file, dir)}
            >
              {node.name}
            </button>
          </li>
        );
      })}
    </ul>
  );
}

function FileList({
  files,
  selectedId,
  onSelect,
  testId,
}: {
  files: FileSummary[];
  selectedId: string | null;
  onSelect(file: FileSummary): void;
  testId: string;
}) {
  return (
    <ul className="luna-web-list" data-testid={testId}>
      {files.map((file) => (
        <li key={file.file_id}>
          <button
            type="button"
            className="luna-web-list__row"
            aria-current={file.file_id === selectedId ? "page" : undefined}
            onClick={() => onSelect(file)}
          >
            <span className="luna-web-list__name">{file.name}</span>
            <span className="luna-web-list__path">{file.path}</span>
          </button>
        </li>
      ))}
    </ul>
  );
}

export function Sidebar({ client, tree, recent, selectedId, selectedPath, onSelect, canCreate, onNewNote, onNewFolder, onRestored }: SidebarProps) {
  const { t } = useTranslation("web");
  const [tab, setTab] = useState<Tab>("tree");
  const [fulltextMode, setFulltextMode] = useState(false);
  const fulltext = useFulltext(client);
  const indexReady = fulltext.state.state === "ready";
  const [folder, setFolder] = useState("");
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<FileSummary[] | null>(null);
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set());

  useEffect(() => {
    let active = true;
    if (!query.trim() || fulltextMode) {
      setResults(null);
      return;
    }
    void client.call("search_titles", { query }).then(
      (hits) => {
        if (active) setResults(hits);
      },
      () => {
        if (active) setResults([]);
      },
    );
    return () => {
      active = false;
    };
  }, [client, query, tree, fulltextMode]);

  useEffect(() => {
    if (!selectedPath) return;
    const parts = selectedPath.split("/").slice(0, -1);
    setExpanded((current) => {
      const next = new Set(current);
      parts.forEach((_, index) => next.add(parts.slice(0, index + 1).join("/")));
      return next.size === current.size ? current : next;
    });
  }, [selectedPath]);

  function toggle(path: string) {
    setFolder(path);
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  return (
    <nav className="luna-web-sidebar" aria-label={t("nav.label")}>
      <div className="luna-web-sidebar__create">
        <button
          type="button"
          className="luna-web-button"
          disabled={!canCreate}
          title={folder ? t("nav.inFolder", { folder }) : t("nav.inRoot")}
          onClick={() => onNewNote(folder)}
        >
          {t("nav.newNote")}
        </button>
        <button type="button" className="luna-web-button luna-web-button--quiet" disabled={!canCreate} onClick={() => onNewFolder(folder)}>
          {t("nav.newFolder")}
        </button>
      </div>
      <div className="luna-web-sidebar__search">
        <label className="luna-web-sidebar__query">
          <span className="luna-web-visually-hidden">{fulltextMode ? t("search.fulltext") : t("search.label")}</span>
          <input
            className="luna-web-input"
            type="search"
            placeholder={fulltextMode ? t("search.fulltextPlaceholder") : t("search.placeholder")}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
        <button
          type="button"
          className="luna-web-button luna-web-button--quiet"
          aria-pressed={fulltextMode}
          onClick={() => setFulltextMode((on) => !on)}
        >
          {t("search.fulltext")}
        </button>
      </div>
      {fulltextMode && (!indexReady || query.trim()) ? (
        indexReady ? (
          <FulltextResults client={client} query={query} selectedId={selectedId} onSelect={onSelect} />
        ) : (
          <FulltextGate fulltext={fulltext} />
        )
      ) : results !== null ? (
        results.length > 0 ? (
          <FileList files={results} selectedId={selectedId} onSelect={onSelect} testId="search-results" />
        ) : (
          <p className="luna-web-hint">{t("search.empty")}</p>
        )
      ) : (
        <>
          <div className="luna-web-tabs" role="tablist">
            {TABS.map((option) => (
              <button
                key={option}
                type="button"
                role="tab"
                className="luna-web-tabs__tab"
                aria-selected={tab === option}
                onClick={() => setTab(option)}
              >
                {t(`nav.${option}`)}
              </button>
            ))}
          </div>
          {tab === "tags" ? (
            indexReady ? (
              <TagPanel client={client} selectedId={selectedId} onSelect={onSelect} />
            ) : (
              <FulltextGate fulltext={fulltext} />
            )
          ) : tab === "trash" ? (
            <RecentlyDeleted client={client} canRestore={canCreate} onRestored={onRestored} />
          ) : tab === "tree" ? (
            tree.length > 0 ? (
              <div className="luna-web-tree" data-testid="note-tree">
                <TreeLevel
                  nodes={tree}
                  dir=""
                  depth={0}
                  expanded={expanded}
                  onToggle={toggle}
                  selectedId={selectedId}
                  onSelect={(file, dir) => {
                    setFolder(dir);
                    onSelect(file);
                  }}
                />
              </div>
            ) : (
              <p className="luna-web-hint">{t("tree.empty")}</p>
            )
          ) : (
            <FileList files={recent} selectedId={selectedId} onSelect={onSelect} testId="recent-list" />
          )}
        </>
      )}
    </nav>
  );
}
