// 搜索视图（PRD-06-031，Obsidian 搜索面板对标）+ 跨库查找替换（PRD-06-014）。
//
// 落位在**主侧栏**而非独立页面：搜索是"在写东西的过程中找东西"，主区那片编辑工作面
// 必须留着——「搜索」这一级空间与「笔记 / 全部笔记」共用同一片编辑面，判据在
// `workspace-ia.ts` 的 `WORKSPACE_IA`（PRD-07-058）。点一条命中即在当前 pane 打开
// 该笔记并跳到该行。
//
// 呈现层不碰语法也不算偏移：命中行由 Rust 下发为 `{text, hit}` 分段序列，这里只负责
// 把 `hit` 段包成 `<mark>`。

import { useMemo, useState, type MouseEvent as ReactMouseEvent } from "react";
import { IconButton } from "../../components/ui/IconButton";
import { useTranslation } from "react-i18next";
import type { MatchSegmentDto, ReplaceNoteDto, SearchNoteDto } from "../../ipc/bindings";
import {
  ChevronDownIcon,
  ChevronRightIcon,
  CloseIcon,
  CopyIcon,
  FileIcon,
  FoldIcon,
  InfoIcon,
  ReplaceIcon,
  SearchIcon,
} from "@lunarca/editor-core/Icon";
import { QueryBox } from "../../components/ui/QueryBox";
import { commands } from "../../ipc/bindings";
import { useContextMenu } from "../../components/ContextMenuHost";
import { formatShortcut } from "@lunarca/editor-core/shortcut-display";
import { buildMenu, separator } from "../../lib/context-menu";
import { useNoteMenu } from "../workspace/note-menu";
import { requestEditorReveal } from "../../lib/app-events";
import type { VaultSearchController } from "./use-vault-search";
import { EmptyState } from "../../components/ui/EmptyState";
import { useIndexStatus } from "../../hooks/use-index-status";
import { SEARCH_SYNTAX_HINT_IDS } from "./search-syntax";
import { groupSearchHits } from "./search-groups";

export interface SearchViewProps {
  /** 全库搜索的键位（设置真值，可改键；空 = 没绑键，就不显示提示）。 */
  searchShortcut: string;
  controller: VaultSearchController;
  /** 在当前 pane 打开一篇笔记（随后由 reveal 事件跳到命中行）。 */
  onOpenPath: (path: string) => void;
  /** 在右侧分屏打开（右键菜单；「照着这篇改那篇」是搜索最常见的下一步）。 */
  onOpenInSplit?: (path: string) => void;
  /** 在独立窗口打开（右键菜单）。 */
  onOpenInWindow?: (path: string) => void;
  /** 状态回流（复制/导出等动作的反馈）。 */
  onStatus?: (message: string) => void;
}

function Highlighted({ segments }: { segments: MatchSegmentDto[] }) {
  return (
    <>
      {segments.map((segment, index) =>
        segment.hit ? (
          <mark key={index} className="luna-search__hit">
            {segment.text}
          </mark>
        ) : (
          <span key={index}>{segment.text}</span>
        ),
      )}
    </>
  );
}

function noteDir(path: string): string {
  const index = path.lastIndexOf("/");
  return index > 0 ? path.slice(0, index) : "";
}

export function SearchView({
  controller,
  searchShortcut,
  onOpenPath,
  onOpenInSplit,
  onOpenInWindow,
  onStatus,
}: SearchViewProps) {
  const { t } = useTranslation(["workspace", "common"]);
  const indexStatus = useIndexStatus();
  const contextMenu = useContextMenu();
  const noteMenu = useNoteMenu();
  const [syntaxOpen, setSyntaxOpen] = useState(false);
  /** 「资料」分组默认收起（ADR-024 降权呈现）；展开态在同一查询会话内保持。 */
  const [materialOpen, setMaterialOpen] = useState(false);
  /**
   * 「记录」分组默认**展开**（PRD-06-052）——与资料组同形态、但**不同默认**。
   *
   * 分组只回答「这是草稿」，不惩罚它的排序：记录是用户之言，「我上周记了什么」与
   * 找一篇知识笔记同等常见。默认收起就是把「降权」从排序偷渡进呈现——用户得多点一下
   * 才看得到自己写过的东西，而 Rust 侧刚刚才明确不给它降权。
   */
  const [draftOpen, setDraftOpen] = useState(true);
  const {
    query,
    setQuery,
    matchCase,
    toggleMatchCase,
    results,
    loading,
    error,
    collapsed,
    toggleCollapsed,
    collapseAll,
    expandAll,
    replaceOpen,
    setReplaceOpen,
    replacement,
    setReplacement,
    replaceStage,
    replaceBusy,
    previewReplace,
    applyReplace,
    resetReplace,
    refresh,
  } = controller;

  /** 替换预览按路径索引，供命中行就地切换成 before/after 对照。 */
  const previewByPath = useMemo(() => {
    const map = new Map<string, ReplaceNoteDto>();
    if (replaceStage.kind === "preview") {
      for (const note of replaceStage.plan.notes) {
        map.set(note.path, note);
      }
    }
    return map;
  }, [replaceStage]);

  function openHit(path: string, line: number) {
    onOpenPath(path);
    // 暂存 + 广播（app-events 的成对机制）：目标笔记未打开时纯事件必然丢失——
    // 监听器在派发那一刻还持着旧 pathRef，症状是"点了命中只打开、不跳行"。
    requestEditorReveal({ path, line, headingIndex: null, focusEditor: true });
  }

  /** 命中笔记右键：打开方式 + 折叠 + 逐篇替换（有预览时）+ 通用笔记动作。 */
  function openNoteMenu(event: ReactMouseEvent, note: SearchNoteDto) {
    const isCollapsed = collapsed.has(note.path);
    const preview = previewByPath.get(note.path);
    contextMenu.open(
      event,
      noteMenu({
        path: note.path,
        openInNotes: () => openHit(note.path, note.lines[0]?.line ?? 1),
        openInSplit: onOpenInSplit ? () => onOpenInSplit(note.path) : undefined,
        openInWindow: onOpenInWindow ? () => onOpenInWindow(note.path) : undefined,
        leading: [
          {
            kind: "action",
            id: "toggleCollapse",
            label: t(isCollapsed ? "search.expandNote" : "search.collapseNote"),
            icon: isCollapsed ? <ChevronRightIcon size={13} /> : <ChevronDownIcon size={13} />,
            onSelect: () => toggleCollapsed(note.path),
          },
          preview
            ? {
                kind: "action",
                id: "replaceNote",
                label: t("search.replaceThisNote"),
                icon: <ReplaceIcon size={13} />,
                disabled: replaceBusy,
                onSelect: () => void applyReplace(note.path),
              }
            : null,
        ],
        onStatus: onStatus ?? (() => {}),
        // 改名会让这一条命中的路径变掉——不重跑这次查询，列表上留着的就是一条点不开的
        // 旧路径（结果是一次查询的快照，不随 `vault://changed` 自己刷新）。
        onChanged: refresh,
      }),
    );
  }

  /** 命中行右键：跳到该行 / 复制这一行原文（复制的是磁盘上的那一行，不是高亮片段）。 */
  function openLineMenu(event: ReactMouseEvent, path: string, line: number, text: string) {
    contextMenu.open(
      event,
      buildMenu([
        {
          kind: "action",
          id: "open",
          label: t("common:action.open"),
          icon: <FileIcon size={13} />,
          onSelect: () => openHit(path, line),
        },
        {
          kind: "action",
          id: "copyLine",
          label: t("search.copyLine"),
          icon: <CopyIcon size={13} />,
          onSelect: () => {
            void commands.popupCopy(text);
          },
        },
      ]),
    );
  }

  /** 结果区空白处右键：作用于整份结果的动作。 */
  function openViewMenu(event: ReactMouseEvent) {
    contextMenu.open(
      event,
      buildMenu([
        {
          kind: "action",
          id: "collapseAll",
          label: t("search.collapseAll"),
          icon: <FoldIcon size={13} />,
          disabled: noteCount === 0,
          onSelect: collapseAll,
        },
        {
          kind: "action",
          id: "expandAll",
          label: t("search.expandAll"),
          icon: <ChevronDownIcon size={13} />,
          disabled: noteCount === 0 || collapsed.size === 0,
          onSelect: expandAll,
        },
        separator("query"),
        {
          kind: "action",
          id: "toggleReplace",
          label: t("search.replace"),
          icon: <ReplaceIcon size={13} />,
          checked: replaceOpen,
          onSelect: () => {
            setReplaceOpen(!replaceOpen);
            resetReplace();
          },
        },
        {
          kind: "action",
          id: "clear",
          label: t("search.clear"),
          icon: <CloseIcon size={13} />,
          disabled: !hasQuery,
          onSelect: () => setQuery(""),
        },
      ]),
      { highlight: null },
    );
  }

  function renderNote(note: SearchNoteDto) {
    const isCollapsed = collapsed.has(note.path);
    const preview = previewByPath.get(note.path);
    const dir = noteDir(note.path);
    // 命中行与替换预览行归一成同一形状：`after` 有值即渲染 before/after 对照。
    const rows = preview
      ? preview.lines.map((line) => ({
          line: line.line,
          segments: line.before,
          after: line.after as string | null,
        }))
      : note.lines.map((line) => ({
          line: line.line,
          segments: line.segments,
          after: null as string | null,
        }));
    return (
      <li key={note.path} className="luna-search__note">
        <div className="luna-search__note-head">
          <IconButton
            className="luna-search__note-toggle"
            aria-expanded={!isCollapsed}
            label={t(isCollapsed ? "search.expandNote" : "search.collapseNote")}
            icon={isCollapsed ? <ChevronRightIcon size={14} /> : <ChevronDownIcon size={14} />}
            onClick={() => toggleCollapsed(note.path)}
          />
          <button
            type="button"
            className="luna-search__note-open"
            onClick={() => openHit(note.path, note.lines[0]?.line ?? 1)}
            onContextMenu={(event) => openNoteMenu(event, note)}
            title={note.path}
          >
            <FileIcon size={14} />
            <span className="luna-search__note-title">{note.title}</span>
            {dir && <span className="luna-search__note-dir">{dir}</span>}
          </button>
          <span className="luna-count-badge">{note.match_count}</span>
          {preview && (
            <button
              type="button"
              className="luna-search__note-apply"
              disabled={replaceBusy}
              title={t("search.replaceThisNote")}
              aria-label={t("search.replaceThisNote")}
              onClick={() => void applyReplace(note.path)}
            >
              <ReplaceIcon size={13} />
            </button>
          )}
        </div>
        {!isCollapsed && (
          <ul className="luna-search__lines">
            {rows.map((row) => (
              <li key={row.line}>
                <button
                  type="button"
                  className="luna-search__line"
                  onClick={() => openHit(note.path, row.line)}
                  onContextMenu={(event) =>
                    openLineMenu(
                      event,
                      note.path,
                      row.line,
                      row.segments.map((segment) => segment.text).join(""),
                    )
                  }
                >
                  <span className="luna-search__line-no">{row.line}</span>
                  <span className="luna-search__line-text">
                    <Highlighted segments={row.segments} />
                  </span>
                </button>
                {row.after !== null && (
                  <div className="luna-search__line-after">
                    <span className="luna-search__line-no" aria-hidden="true" />
                    <span className="luna-search__line-text">{row.after}</span>
                  </div>
                )}
              </li>
            ))}
            {note.lines_truncated && (
              <li className="luna-search__more">{t("search.moreMatches")}</li>
            )}
          </ul>
        )}
      </li>
    );
  }

  const hasQuery = query.trim().length > 0;
  const noteCount = results?.notes.length ?? 0;
  // 分桶口径归 `groupSearchHits`（可无头测试的纯判定，判据全由 Rust 下发）。
  const hits = groupSearchHits(results?.notes ?? []);

  /**
   * 一个可折叠命中分组（组头 + 计数 + 命中列表）——「记录」与「资料」**共用这一份**。
   *
   * 两处各画一份必然漂移（组头的图标尺寸、计数徽标的类名、展开态的 aria 都要对得上），
   * 而这两个组在版式上必须是对等的两段：它们回答的是同一个问题的两个答案。
   */
  function renderHitGroup(
    labelKey: string,
    hits: readonly SearchNoteDto[],
    open: boolean,
    setOpen: (next: boolean) => void,
  ) {
    if (hits.length === 0) {
      return null;
    }
    return (
      <section className="luna-search__group">
        <button
          type="button"
          className="luna-search__group-head"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
        >
          {open ? <ChevronDownIcon size={14} /> : <ChevronRightIcon size={14} />}
          <span className="luna-search__group-title">{t(labelKey)}</span>
          <span className="luna-count-badge">{hits.length}</span>
        </button>
        {open && <ul className="luna-search__notes">{hits.map(renderNote)}</ul>}
      </section>
    );
  }

  /** 同一份语法 DOM：空态经原生 disclosure 按需展开，有查询时由工具行切换。 */
  function renderSyntax() {
    return (
      <div className="luna-search__syntax">
        <dl className="luna-search__syntax-list">
          {SEARCH_SYNTAX_HINT_IDS.map((id) => (
            <div key={id} className="luna-search__syntax-row">
              <dt>
                <code>{t(`search.syntaxSamples.${id}`)}</code>
              </dt>
              <dd>{t(`search.syntax.${id}`)}</dd>
            </div>
          ))}
        </dl>
        <p className="luna-search__syntax-note">{t("search.syntaxLimits")}</p>
      </div>
    );
  }

  return (
    <section className="luna-search" aria-label={t("search.title")} onContextMenu={openViewMenu}>
      <header className="luna-search__header luna-sidebar-header">
        {/* 与其余侧栏同一页头：计数行在上、不带标题（空间名在面包屑里），查询框是这一格
            的"主动作"，占主按钮那一行（PRD-07-071 ⑦）。 */}
        <div className="luna-sidebar-header__title">
          {results && <span className="luna-count-badge">{noteCount}</span>}
        </div>
        <QueryBox
          value={query}
          onChange={setQuery}
          placeholder={t("search.placeholder")}
          ariaLabel={t("search.title")}
          autoFocus
        >
          <button
            type="button"
            className="luna-query-box__action luna-query-box__action--text"
            aria-pressed={matchCase}
            title={t("search.matchCase")}
            aria-label={t("search.matchCase")}
            onClick={toggleMatchCase}
          >
            {t("search.matchCaseIndicator")}
          </button>
        </QueryBox>
        {/* 工具行只在有查询时出现：无查询时「替换」没有对象、「折叠全部」没有内容，
            语法入口已随空态就近提供——三个按钮同时出现只会制造噪声。 */}
        {hasQuery && (
          <div className="luna-search__actions">
            <button
              type="button"
              className="luna-search__action"
              aria-pressed={replaceOpen}
              onClick={() => {
                setReplaceOpen(!replaceOpen);
                resetReplace();
              }}
            >
              <ReplaceIcon size={14} />
              <span>{t("search.replace")}</span>
            </button>
            <button
              type="button"
              className="luna-search__action"
              aria-pressed={syntaxOpen}
              title={t("search.syntaxTitle")}
              aria-label={t("search.syntaxTitle")}
              onClick={() => setSyntaxOpen(!syntaxOpen)}
            >
              <InfoIcon size={14} />
            </button>
            {noteCount > 0 && (
              <button
                type="button"
                className="luna-search__action"
                title={t(collapsed.size > 0 ? "search.expandAll" : "search.collapseAll")}
                aria-label={t(collapsed.size > 0 ? "search.expandAll" : "search.collapseAll")}
                onClick={() => (collapsed.size > 0 ? expandAll() : collapseAll())}
              >
                <FoldIcon size={14} />
              </button>
            )}
          </div>
        )}
        {hasQuery && syntaxOpen && renderSyntax()}
        {replaceOpen && (
          <div className="luna-search__replace">
            {/* 替换词是一个独立输入框，不是查询框的一部分：此前它借查询框内部的
                `.luna-search__input`（无边框、透明底）再补一层边框补回来，等于绕一圈
                得到 `.luna-input`。 */}
            <input
              type="text"
              className="luna-input"
              value={replacement}
              placeholder={t("search.replacePlaceholder")}
              aria-label={t("search.replace")}
              onChange={(event) => setReplacement(event.target.value)}
            />
            {results && !results.replaceable ? (
              <p className="luna-search__hint">{t("search.replaceNeedsSingleTerm")}</p>
            ) : (
              <div className="luna-search__replace-actions">
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary"
                  disabled={!hasQuery || replaceBusy || !results?.replaceable}
                  onClick={() => void previewReplace()}
                >
                  {t("search.previewReplace")}
                </button>
                {replaceStage.kind === "preview" && (
                  <button
                    type="button"
                    className="luna-btn luna-btn--primary"
                    disabled={replaceBusy || replaceStage.plan.total_matches === 0}
                    onClick={() => void applyReplace()}
                  >
                    {t("search.applyReplaceAll", {
                      matches: replaceStage.plan.total_matches,
                      notes: replaceStage.plan.notes.length,
                    })}
                  </button>
                )}
              </div>
            )}
            {replaceStage.kind === "preview" && (
              <p className="luna-search__hint">{t("search.replacePreviewHint")}</p>
            )}
            {replaceStage.kind === "done" && (
              <p className="luna-search__done">
                {t("search.replaceDone", {
                  matches: replaceStage.plan.total_matches,
                  notes: replaceStage.plan.notes.length,
                })}
              </p>
            )}
            {replaceStage.kind === "done" &&
              replaceStage.plan.failures.map((failure) => (
                <p key={failure.path} className="luna-field-error">
                  {failure.path}: {failure.error}
                </p>
              ))}
          </div>
        )}
      </header>

      {error && (
        <p className="luna-field-error" role="alert">
          {error}
        </p>
      )}
      {!hasQuery && (
        <EmptyState
          className="luna-search__start"
          title={t("search.emptyIdleTitle")}
          description={t("search.emptyIdle")}
          footnote={
            searchShortcut ? (
              <>
                {t("search.shortcutHint")}
                <kbd>{formatShortcut(searchShortcut)}</kbd>
              </>
            ) : undefined
          }
        >
          <details className="luna-search__syntax-disclosure">
            <summary>
              <InfoIcon size={14} />
              <span>{t("search.syntaxTitle")}</span>
            </summary>
            {renderSyntax()}
          </details>
        </EmptyState>
      )}
      {/* 开库后的增量扫描还没做完：结果可能不全，明说，不拿空结果冒充「没有」（PRD-06-004 ④）。 */}
      {hasQuery && indexStatus && (
        <p className="luna-search__summary" role="status">
          {t("common:status.indexingPartial", { indexed: indexStatus.indexed, total: indexStatus.total })}
        </p>
      )}
      {hasQuery && loading && (
        <p className="luna-search__status" role="status" aria-live="polite">
          {t("common:status.loading")}
        </p>
      )}
      {hasQuery && !loading && results && noteCount === 0 && (
        <EmptyState
          icon={<SearchIcon size={24} />}
          title={t("search.emptyNoResultTitle")}
          description={t("search.emptyNoResult")}
          action={
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              onClick={() => setQuery("")}
            >
              {t("search.clearQuery")}
            </button>
          }
        />
      )}
      {results && noteCount > 0 && (
        <>
          <p className="luna-search__summary" role="status" aria-live="polite">
            {/* 纯过滤器查询（`tag:工作`）没有正文命中，说「0 处命中」是错的：它筛的是笔记不是词。 */}
            {results.total_matches > 0
              ? t("search.summary", { matches: results.total_matches, notes: noteCount })
              : t("search.summaryNotesOnly", { notes: noteCount })}
          </p>
          {/* 「笔记」在前正常展开；其后是**成熟度介于两者之间**的两个分组，顺序即口径：
              「记录」（用户之言、未定稿，不降权故默认展开）在前，「资料」（录音/转写/对话，
              ADR-024 降权故默认收起）在后——它们的命中有真实价值（"那句话是在哪次会上说的"），
              但不该把知识笔记刷下屏幕。 */}
          <ul className="luna-search__notes">{hits.knowledge.map(renderNote)}</ul>
          {renderHitGroup("search.draftGroup", hits.draft, draftOpen, setDraftOpen)}
          {renderHitGroup("search.materialGroup", hits.material, materialOpen, setMaterialOpen)}
          {results.truncated && <p className="luna-search__hint">{t("search.truncated")}</p>}
        </>
      )}
    </section>
  );
}
