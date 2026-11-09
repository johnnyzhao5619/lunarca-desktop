/**
 * 网页上的一篇笔记（PRD-21-020 / 021 / 030 / 031）：与桌面同一套编辑器（`@lunarca/editor-core/markdown-editor`），
 * 三种模式与桌面同名同序（实时预览 / 源码 / 阅读，默认实时预览），与桌面同一条格式工具条与斜杠菜单（桌面专属的
 * AI、模板与附件条目不出现），自动保存与四种保存状态、别处改了的横幅（「查看对比」是逐行差异，算法在 WASM，
 * 与桌面同一份）、已合并的提示（可撤回）、冲突副本、只读（写入闸）。保存与合并的编排在 `NoteSession`，这里只渲染。
 *
 * 一个编辑器实例跨笔记复用：换篇时把上一篇的状态收回它的 `NoteSession`（没存上的改动、撤销历史、光标都
 * 跟着它），换上这一篇的。
 */

import { chooseFiles } from "../media/choose-files";
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import { history } from "@codemirror/commands";
import { Compartment, EditorState, Transaction, type Extension } from "@codemirror/state";
import { EditorView, placeholder } from "@codemirror/view";
import { ReadingView } from "@lunarca/editor-core/ReadingView";
import { DiffView, type LineDiff } from "@lunarca/editor-core/DiffView";
import { runEditorFormatAction, type EditorActionDeps, type EditorFormatAction } from "@lunarca/editor-core/editor-actions";
import { formatStateAt, type FormatState } from "@lunarca/editor-core/format-commands";
import { FormatToolbar } from "@lunarca/editor-core/FormatToolbar";
import { editorPhrases, livePreviewExtensions, markdownEditorExtensions } from "@lunarca/editor-core/markdown-editor";
import type { SlashCommand } from "@lunarca/editor-core/slash-commands";
import { webMarkdownHost } from "../../lib/markdown-host";
import { saveErrorText } from "../../lib/error-message";
import type { FileSummary, WrittenFile } from "../../worker/protocol";
import { WorkerError, type WorkerClient } from "../../worker/worker-client";
import { webReadingVault } from "../reader/web-reading-vault";
import { deleteAllowed, EDITOR_MODES, type EditorMode } from "./editor-mode";
import type { NoteSession, SessionSnapshot } from "./note-session";
import type { SessionRegistry } from "./session-registry";
import { subscribeTabFocusMode, tabFocusModeEnabled } from "./tab-focus-mode";
import type { WebEditorHost } from "./web-editor-host";
import { webEditorUi } from "./web-editor-ui";

/** 「已合并别处的改动」提示停留多久（之后撤回就不再提供）。 */
const MERGED_NOTICE_MS = 10_000;

const IDLE_SNAPSHOT: SessionSnapshot = { status: { kind: "saved" }, remoteRev: null, gone: false, merged: null, conflictCopy: null };
const NO_SUBSCRIPTION = () => () => {};
const NO_FORMAT: FormatState = { strong: false, em: false, del: false, code: false, heading: 0, bullet: false, ordered: false, task: false, quote: false };

/** 桌面专属的斜杠条目：AI、模板（模板库只在桌面）。插入附件网页也有（选文件即上传，PRD-21-025）。 */
function desktopOnly(command: SlashCommand): boolean {
  return command.action.kind === "ai" || command.action.kind === "template";
}

export interface NoteEditorProps {
  client: WorkerClient;
  host: WebEditorHost;
  registry: SessionRegistry;
  file: FileSummary;
  mode: EditorMode;
  onModeChange(mode: EditorMode): void;
  /** 写入闸的原因码；`null` = 可写。 */
  readOnly: string | null;
  onOpenPath(path: string): void;
  /** 打开后才知道这个文件不是文本（扩展名认不出来的附件）：交回工作区，换成能下载的附件页。 */
  onNotText(fileId: string): void;
  /** 两边改了同一段 / 另存了我的这份：把副本并排打开。 */
  onConflictCopy(copy: WrittenFile): void;
  onRename(file: FileSummary): void;
  onMove(file: FileSummary): void;
  onDelete(file: FileSummary, session: NoteSession | null): void;
}

export function NoteEditor(props: NoteEditorProps) {
  const { client, host, registry, file, mode, readOnly } = props;
  const { t, i18n } = useTranslation("web");
  const surfaceRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const extensionsRef = useRef<Extension[]>([]);
  const [compartments] = useState(() => ({
    history: new Compartment(),
    readOnly: new Compartment(),
    locale: new Compartment(),
    presentation: new Compartment(),
  }));
  const [session, setSession] = useState<NoteSession | null>(null);
  const sessionRef = useRef<NoteSession | null>(null);
  const [loadProblem, setLoadProblem] = useState<string | null>(null);
  const [docVersion, setDocVersion] = useState(0);
  const [formatState, setFormatState] = useState<FormatState>(NO_FORMAT);
  /** 「查看对比」打开时的逐行差异（旧 = 别处那一版，新 = 编辑器里的）。 */
  const [comparison, setComparison] = useState<LineDiff | null>(null);
  const readingVault = useMemo(() => webReadingVault(client), [client]);

  // 扩展闭包读「此刻」的值（扩展只建一次）。
  const live = useRef({ mode, props });
  live.current = { mode, props };

  const buildPresentation = useCallback(
    (): Extension =>
      livePreviewExtensions(host, {
        t,
        notePath: () => sessionRef.current?.path ?? null,
        onOpenPath: (path) => live.current.props.onOpenPath(path),
        onActivateWikiLink: (target) => {
          const from = sessionRef.current?.path;
          if (from) host.vault.openWikiLink(from, target, (path) => live.current.props.onOpenPath(path));
        },
        // 网页没有标签视图：点标签不跳转。
        onOpenTag: () => {},
        onEditSource: (view, line) => {
          live.current.props.onModeChange("source");
          const at = view.state.doc.line(Math.min(line, view.state.doc.lines)).from;
          view.dispatch({ selection: { anchor: at }, scrollIntoView: true });
          view.focus();
        },
      }),
    [host, t],
  );

  const localeExtension = useCallback((): Extension => [editorPhrases(t), placeholder(t("note.placeholder"))], [t]);

  /**
   * 格式工具条与斜杠菜单的动作（与桌面同一个分发 `runEditorFormatAction`）。插入附件 = 选文件即上传；
   * 桌面专属的那几样（模板、分列、粘贴）网页不提供入口，这里给的是「什么都不做」的那一面；做完把焦点还给正文。
   */
  const runAction = useCallback(
    (action: EditorFormatAction) => {
      const view = viewRef.current;
      if (!view || !sessionRef.current || live.current.props.readOnly !== null) return;
      const deps: EditorActionDeps = {
        linkPlaceholder: t("editor-core:editor.linkPlaceholder"),
        copyText: (text) => navigator.clipboard.writeText(text),
        paste: async () => {},
        pickAttachments: async () =>
          host.uploadFiles(
            { notePath: () => sessionRef.current?.path ?? null, readOnly: () => live.current.props.readOnly },
            await chooseFiles(),
          ),
        pickTemplate: async () => null,
        convertTextToTable: async () => null,
      };
      void runEditorFormatAction(view, action, deps).then(() => view.focus());
    },
    [t, host],
  );

  /** 换了状态（换篇）或模式 / 只读 / 语言变了：把四个隔间配成此刻该有的样子。 */
  const syncCompartments = useCallback(() => {
    const view = viewRef.current;
    if (!view) return;
    const current = live.current;
    view.dispatch({
      effects: [
        compartments.presentation.reconfigure(current.mode === "live" ? buildPresentation() : []),
        compartments.readOnly.reconfigure(EditorState.readOnly.of(current.props.readOnly !== null || sessionRef.current === null)),
        compartments.locale.reconfigure(localeExtension()),
      ],
    });
  }, [buildPresentation, compartments, localeExtension]);

  // 编辑器只建一次。
  useEffect(() => {
    const surface = surfaceRef.current;
    if (!surface) return;
    extensionsRef.current = markdownEditorExtensions(host, {
      t,
      notePath: () => sessionRef.current?.path ?? null,
      history: compartments.history.of(history()),
      readOnly: compartments.readOnly.of(EditorState.readOnly.of(true)),
      locale: compartments.locale.of(localeExtension()),
      presentation: compartments.presentation.of(live.current.mode === "live" ? buildPresentation() : []),
      slash: {
        label: (key) => t(`editor-core:${key}`),
        run: (_view, command) => {
          if (command.action.kind !== "ai") runAction(command.action);
        },
        hidden: desktopOnly,
      },
      slots: {
        afterCompletions: host.uploadExtension({
          notePath: () => sessionRef.current?.path ?? null,
          readOnly: () => live.current.props.readOnly,
        }),
        beforeBaseKeymap: [
          EditorView.domEventHandlers({
            blur: () => {
              void sessionRef.current?.flush();
            },
          }),
          EditorView.updateListener.of((update) => {
            if (update.docChanged || update.selectionSet) setFormatState(formatStateAt(update.state));
            const current = sessionRef.current;
            if (!current || !update.docChanged) return;
            const userEdit = update.transactions.some(
              (tr) => tr.docChanged && tr.annotation(Transaction.addToHistory) !== false,
            );
            current.docChanged(update.changes, userEdit);
            if (live.current.mode === "reading") setDocVersion((n) => n + 1);
          }),
        ],
      },
    });
    const view = new EditorView({
      parent: surface,
      state: EditorState.create({ doc: "", extensions: extensionsRef.current }),
    });
    viewRef.current = view;
    if (tabFocusModeEnabled()) view.setTabFocusMode(true);
    const offTabFocus = subscribeTabFocusMode(() => view.setTabFocusMode(tabFocusModeEnabled()));
    return () => {
      offTabFocus();
      const current = sessionRef.current;
      if (current) {
        current.detach();
        registry.release(current.fileId);
      }
      sessionRef.current = null;
      view.destroy();
      viewRef.current = null;
      host.releaseImages();
    };
    // 编辑器只建一次；会变的东西经 `live` 与隔间进来。
  }, []);

  // 换篇：上一篇收回它的会话（随即冲刷），这一篇从会话里接上，没有就读出来新建。
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    let active = true;
    const attach = (next: NoteSession) => {
      sessionRef.current = next;
      next.attach(view);
      if (tabFocusModeEnabled()) view.setTabFocusMode(true);
      setFormatState(formatStateAt(view.state));
      setSession(next);
      setLoadProblem(null);
      syncCompartments();
      setDocVersion((n) => n + 1);
    };
    const existing = registry.get(file.file_id);
    if (existing && !existing.restored) {
      attach(existing);
    } else {
      sessionRef.current = null;
      setSession(null);
      view.setState(EditorState.create({ doc: "", extensions: extensionsRef.current }));
      syncCompartments();
      client.call("read", { file_id: file.file_id }).then(
        (note) => {
          if (!active) return;
          attach(registry.open(note, EditorState.create({ doc: note.text, extensions: extensionsRef.current })));
        },
        (error: unknown) => {
          if (!active) return;
          const code = error instanceof WorkerError ? error.code : "internal";
          setLoadProblem(code);
          if (code === "not_text") live.current.props.onNotText(file.file_id);
        },
      );
    }
    return () => {
      active = false;
      const current = sessionRef.current;
      if (current && current.fileId === file.file_id) {
        current.detach();
        sessionRef.current = null;
        registry.release(current.fileId);
      }
      host.releaseImages();
    };
  }, [client, file.file_id, host, registry, syncCompartments]);

  useEffect(() => {
    syncCompartments();
    if (mode === "reading") setDocVersion((n) => n + 1);
  }, [mode, readOnly, i18n.language, syncCompartments]);

  const snapshot = useSyncExternalStore(session?.subscribe ?? NO_SUBSCRIPTION, session?.getSnapshot ?? (() => IDLE_SNAPSHOT));

  // 两边改了同一段：副本并排打开（PRD-21-031）。
  const copyId = snapshot.conflictCopy?.file_id ?? null;
  useEffect(() => {
    if (snapshot.conflictCopy) live.current.props.onConflictCopy(snapshot.conflictCopy);
    // 只在出现一份新的副本时打开一次。
  }, [copyId]);

  // 「已合并」提示停一会儿就收起。
  const mergedId = snapshot.merged?.id ?? null;
  useEffect(() => {
    if (mergedId === null || !session) return;
    const timer = window.setTimeout(() => session.dismissMerged(), MERGED_NOTICE_MS);
    return () => window.clearTimeout(timer);
  }, [mergedId, session]);

  // 别处又改了一次、横幅收起或换了篇：上一次的对比已经不是此刻的两版，收起。
  useEffect(() => {
    setComparison(null);
  }, [snapshot.remoteRev, file.file_id]);

  /** 「查看对比」：取别处那一版，与编辑器里的内容逐行比（差异算法在 WASM，与桌面同一份）；再点一次收起。 */
  async function compare() {
    if (comparison) {
      setComparison(null);
      return;
    }
    if (!session) return;
    try {
      const theirs = await client.call("read", { file_id: file.file_id });
      setComparison(await client.call("diff_lines", { old: theirs.text, new: session.text() }));
    } catch {
      // 取不到别处那一版：横幅照旧，合并 / 另存仍可用。
    }
  }

  const comparisonCaption =
    comparison === null
      ? ""
      : comparison.degraded
        ? t("note.compare.degraded")
        : comparison.added === 0 && comparison.removed === 0
          ? t("note.compare.same")
          : t("note.compare.caption");

  async function saveMine() {
    if (!session) return;
    try {
      props.onConflictCopy(await session.saveMineAsCopy());
    } catch {
      // 另存失败：保存状态与横幅照旧，改动仍在编辑器里。
    }
  }

  const status = snapshot.status;
  const statusText =
    status.kind === "failed"
      ? t("note.status.failed", { reason: saveErrorText(t, status.code, status.param, i18n.language) })
      : t(`note.status.${status.kind}`);

  return (
    <section className="luna-web-note" data-testid="note-editor" data-note-path={session?.path ?? file.path}>
      <header className="luna-web-note__head">
        <h2 className="luna-web-note__title" title={session?.path ?? file.path}>
          {file.name}
        </h2>
        <div className="luna-web-note__modes" role="group" aria-label={t("editor-core:editor.mode.label")}>
          {EDITOR_MODES.map((option) => (
            <button
              key={option}
              type="button"
              className="luna-web-note__mode"
              aria-pressed={mode === option}
              onClick={() => props.onModeChange(option)}
            >
              {t(`editor-core:editor.mode.${option}`)}
            </button>
          ))}
        </div>
        <span className="luna-web-note__status" data-testid="save-status" data-status={status.kind} role="status" aria-live="polite">
          {session ? statusText : ""}
        </span>
        <div className="luna-web-note__actions">
          <button type="button" className="luna-web-button luna-web-button--quiet" disabled={readOnly !== null || !session} onClick={() => props.onRename(file)}>
            {t("note.actions.rename")}
          </button>
          <button type="button" className="luna-web-button luna-web-button--quiet" disabled={readOnly !== null || !session} onClick={() => props.onMove(file)}>
            {t("note.actions.move")}
          </button>
          {deleteAllowed(readOnly) ? (
            <button type="button" className="luna-web-button luna-web-button--quiet" disabled={!session} onClick={() => props.onDelete(file, session)}>
              {t("note.actions.delete")}
            </button>
          ) : null}
        </div>
      </header>

      {mode !== "reading" && session && readOnly === null ? (
        <FormatToolbar formatState={formatState} onAction={runAction} ui={webEditorUi} wide attachments />
      ) : null}

      {readOnly !== null ? (
        <p className="luna-web-banner" data-testid="read-only-banner" role="status">
          {t(`note.readOnly.${readOnly}`, { defaultValue: t("note.readOnly.generic", { code: readOnly }) })}
        </p>
      ) : null}
      {snapshot.gone ? (
        <p className="luna-web-banner" data-testid="gone-banner" role="status">
          {t("note.gone")}
        </p>
      ) : null}
      {snapshot.remoteRev !== null ? (
        <div className="luna-web-banner" data-testid="remote-banner" role="alert">
          <span className="luna-web-banner__text">{t("note.remote.body")}</span>
          <span className="luna-web-banner__actions">
            <button type="button" className="luna-web-button" aria-expanded={comparison !== null} onClick={() => void compare()}>
              {comparison ? t("note.remote.hideCompare") : t("note.remote.compare")}
            </button>
            <button type="button" className="luna-web-button" onClick={() => void session?.merge()}>
              {t("note.remote.merge")}
            </button>
            <button type="button" className="luna-web-button" onClick={() => void saveMine()}>
              {t("note.remote.saveMine")}
            </button>
          </span>
        </div>
      ) : null}
      {snapshot.remoteRev !== null && comparison ? (
        <div className="luna-web-note__compare" data-testid="remote-diff">
          <p className="luna-web-hint">{comparisonCaption}</p>
          <DiffView diff={comparison} />
        </div>
      ) : null}
      {snapshot.conflictCopy ? (
        <div className="luna-web-banner" data-testid="conflict-banner" role="status">
          <span className="luna-web-banner__text">{t("note.conflict.body", { path: snapshot.conflictCopy.path })}</span>
          <span className="luna-web-banner__actions">
            <button type="button" className="luna-web-button" onClick={() => snapshot.conflictCopy && props.onConflictCopy(snapshot.conflictCopy)}>
              {t("note.conflict.show")}
            </button>
            <button type="button" className="luna-web-button luna-web-button--quiet" onClick={() => session?.dismissConflict()}>
              {t("note.conflict.dismiss")}
            </button>
          </span>
        </div>
      ) : null}
      {loadProblem ? (
        <p className="luna-web-error" role="alert">
          {saveErrorText(t, loadProblem, undefined, i18n.language)}
        </p>
      ) : null}
      {!session && !loadProblem ? <p className="luna-web-hint">{t("reader.loading")}</p> : null}

      <div className="luna-web-note__body">
        <div ref={surfaceRef} className="luna-web-note__surface luna-md-surface" data-view-mode={mode} hidden={mode === "reading"} />
        {mode === "reading" && session ? (
          <article className="luna-web-reader__note" data-note-path={session.path} data-doc-version={docVersion}>
            <ReadingView
              host={webMarkdownHost}
              vault={readingVault}
              content={session.text()}
              notePath={session.path}
              onOpenPath={props.onOpenPath}
              collapseProperties
            />
          </article>
        ) : null}
      </div>

      {snapshot.merged ? (
        <div className="luna-web-toast" data-testid="merged-toast" role="status">
          <span>{t("note.merged.body")}</span>
          <button type="button" className="luna-web-button" onClick={() => void session?.undoMerge()}>
            {t("note.merged.undo")}
          </button>
          <button type="button" className="luna-web-button luna-web-button--quiet" onClick={() => session?.dismissMerged()}>
            {t("note.merged.dismiss")}
          </button>
        </div>
      ) : null}
    </section>
  );
}
