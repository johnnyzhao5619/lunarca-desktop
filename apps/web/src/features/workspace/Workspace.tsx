/**
 * 已登录的工作区（PRD-21 §3.2–§3.4）：页头（库、账户、退出）、侧栏（搜索 / 文件夹 / 最近、新建）、主区（笔记的
 * 编辑器或附件）与页脚；空闲锁定在这里计时。小屏单栏：打开一篇时侧栏让位，「返回笔记列表」回去。
 *
 * 本页打开过、没存完的笔记都在 `SessionRegistry` 里：页面隐藏 / `pagehide` / 锁定 / 退出前一次冲刷；有没存上
 * 的改动时 `beforeunload` 拦截；会话在服务端结束时由 `App` 经 `unsavedProbe` 问到它们（PRD-21-004）。
 */

import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import { classifyAssetTarget } from "@lunarca/editor-core/asset-kind";
import { AccountPage } from "../account/AccountPage";
import { useDialogs } from "../dialog/DialogHost";
import { DEFAULT_EDITOR_MODE, type EditorMode } from "../editor/editor-mode";
import { NotePanes } from "../editor/NotePanes";
import type { NoteSession } from "../editor/note-session";
import { SessionRegistry, type UnsavedDraft } from "../editor/session-registry";
import { webEditorHost } from "../editor/web-editor-host";
import { HistoryPanel } from "../library/HistoryPanel";
import type { UploadStatus } from "../media/upload-extension";
import { NoteReader } from "../reader/NoteReader";
import { idleLockMs, readIdleLockSetting, type IdleLockSetting } from "../session/idle-lock";
import { useIdleTimer } from "../session/use-idle-timer";
import { readLastVault, writeLastVault } from "../vaults/last-vault";
import { VaultPicker, type VaultListState } from "../vaults/VaultPicker";
import { saveErrorText } from "../../lib/error-message";
import type { FileSummary, OpenedVault, SealedDrafts, SessionSummary, TreeNode, VaultSummary, WrittenFile } from "../../worker/protocol";
import { WorkerError, type WorkerClient } from "../../worker/worker-client";
import { Footer } from "./Footer";
import { Sidebar } from "./Sidebar";

export interface WorkspaceProps {
  client: WorkerClient;
  session: SessionSummary;
  onLock(): void;
  onSignOut(): void;
  /** 会话在服务端结束时，`App` 经它问「还有哪些没存上的改动」。 */
  unsavedProbe: { current: (() => UnsavedDraft[]) | null };
  /** 离线锁定时封存的改动：解锁后打开那个库、解开、接着存（TDD-17 §4）。 */
  sealed: SealedDrafts | null;
  onSealedRestored(): void;
}

function codeOf(error: unknown): { code: string; param?: number } {
  return error instanceof WorkerError ? { code: error.code, param: error.param } : { code: "internal" };
}

/** 树里全部文件夹的路径（「移到…」的下拉框；排序与树同一份由 WASM 给的顺序）。 */
function foldersOf(nodes: readonly TreeNode[], out: string[] = []): string[] {
  for (const node of nodes) {
    if (!node.is_dir) continue;
    out.push(node.path);
    foldersOf(node.children, out);
  }
  return out;
}

/** 树里按 `file_id` 找到这个文件此刻的样子（别处改了名、移了位置）。 */
function findFile(nodes: readonly TreeNode[], fileId: string): FileSummary | null {
  for (const node of nodes) {
    if (node.is_dir) {
      const hit = findFile(node.children, fileId);
      if (hit) return hit;
    } else if (node.file_id === fileId) {
      return { file_id: fileId, path: node.path, name: node.name, size: node.size ?? 0, mtime_ms: node.mtime_ms };
    }
  }
  return null;
}

type View = "notes" | "account";

/** 「附件已上传」停留多久。 */
const UPLOAD_DONE_MS = 4_000;

/** 页面可见时多久确认一次会话还在。 */
const SESSION_CHECK_MS = 60_000;

export function Workspace({ client, session, onLock, onSignOut, unsavedProbe, sealed, onSealedRestored }: WorkspaceProps) {
  const { t, i18n } = useTranslation("web");
  const dialogs = useDialogs();
  const [idleSetting, setIdleSetting] = useState<IdleLockSetting>(() => readIdleLockSetting(window.localStorage));

  const host = useMemo(() => webEditorHost(client), [client]);
  const registry = useMemo(
    () =>
      new SessionRegistry({
        call: (cmd, args) => client.call(cmd, args),
        conflictLabel: () => t("note.conflictLabel"),
        setTimeout: (run, ms) => window.setTimeout(run, ms),
        clearTimeout: (handle) => window.clearTimeout(handle as number),
      }),
    [client],
  );
  useEffect(() => () => registry.dispose(), [registry]);

  // 锁定由服务端执行：先冲刷没存上的改动，再锁（TDD-17 §4）。
  useIdleTimer(idleLockMs(idleSetting, session.keep_signed_in), () => {
    void registry.flushAll().finally(onLock);
  });

  // 页面开着时定期、切回来时立刻看一眼会话：被吊销的页面不留在屏幕上等下一次请求。
  useEffect(() => {
    const check = () => {
      if (document.visibilityState === "visible") void client.call("check_session", {}).catch(() => {});
    };
    const timer = window.setInterval(check, SESSION_CHECK_MS);
    window.addEventListener("focus", check);
    document.addEventListener("visibilitychange", check);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", check);
      document.removeEventListener("visibilitychange", check);
    };
  }, [client]);

  const restoring = useRef(false);
  const [vaults, setVaults] = useState<VaultListState>({ kind: "loading" });
  const [opened, setOpened] = useState<(OpenedVault & { name: string | null }) | null>(null);
  const [openProblem, setOpenProblem] = useState<string | null>(null);
  const [selected, setSelected] = useState<FileSummary | null>(null);
  // 打开才发现不是文本的文件（扩展名认不出来）：记下来走附件页，切走再切回不用再试一次。
  const [notText, setNotText] = useState<ReadonlySet<string>>(() => new Set());
  const [revision, setRevision] = useState(0);
  const [view, setView] = useState<View>("notes");
  const [mode, setMode] = useState<EditorMode>(DEFAULT_EDITOR_MODE);
  const [writeGate, setWriteGate] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  /** 正在看哪一篇的版本历史（PRD-21-026）。 */
  const [historyOf, setHistoryOf] = useState<string | null>(null);
  const upload = useSyncExternalStore(host.uploads.subscribe, host.uploads.snapshot);
  const selectedRef = useRef(selected);
  selectedRef.current = selected;

  useEffect(() => {
    unsavedProbe.current = () => registry.unsavedDrafts();
    return () => {
      unsavedProbe.current = null;
    };
  }, [registry, unsavedProbe]);

  function gate(reason: string | null) {
    setWriteGate(reason);
    registry.setReadOnly(reason);
  }

  async function openVault(vault: VaultSummary): Promise<boolean> {
    setOpenProblem(null);
    setSelected(null);
    try {
      const result = await client.call("open_vault", { vault_id: vault.id });
      writeLastVault(window.localStorage, vault.id);
      setOpened({ ...result, name: vault.name });
      const state = await client.call("write_state", {}).catch(() => null);
      gate(state?.read_only ?? null);
      if (sealed?.vault_id === vault.id && !restoring.current) {
        restoring.current = true;
        registry.restore(await client.call("open_drafts", sealed));
        onSealedRestored();
      }
      return true;
    } catch (error) {
      setOpenProblem(codeOf(error).code);
      return false;
    }
  }

  /** 刚建好的库：列出来、打开、写一篇欢迎笔记并打开它（ADR-178 ③）。 */
  async function enterCreated(id: string) {
    const listed = await client.call("list_vaults", {});
    setVaults({ kind: "ready", vaults: listed });
    const vault = listed.find((item) => item.id === id);
    if (vault && (await openVault(vault))) await createNote("", t("vaults.welcomeBody"));
  }

  /** 用户点「新建笔记库」。 */
  async function createVault() {
    setVaults({ kind: "loading" });
    try {
      const { id } = await client.call("create_vault", { name: t("vaults.defaultName") });
      await enterCreated(id);
    } catch (error) {
      setVaults({ kind: "failed", code: codeOf(error).code });
    }
  }

  async function loadVaults() {
    setVaults({ kind: "loading" });
    try {
      const listed = await client.call("list_vaults", {});
      if (listed.length === 0) {
        if (!(await client.call("sync_entitled", {}))) {
          setVaults({ kind: "not_entitled" });
          return;
        }
        const created = await client.call("create_default_vault", { email: session.account.email, name: t("vaults.defaultName") });
        if (created) {
          await enterCreated(created.id);
          return;
        }
        setVaults({ kind: "ready", vaults: await client.call("list_vaults", {}) });
        return;
      }
      setVaults({ kind: "ready", vaults: listed });
      const last = sealed?.vault_id ?? readLastVault(window.localStorage);
      const remembered = listed.find((vault) => vault.id === last && !vault.locked_reason);
      if (remembered) await openVault(remembered);
    } catch (error) {
      setVaults({ kind: "failed", code: codeOf(error).code });
    }
  }

  useEffect(() => {
    void loadVaults();
    return client.onEvent((event) => {
      if (event.event === "vault_read_only") {
        gate(event.reason);
        return;
      }
      if (event.event !== "vault_changed") return;
      setOpened((current) => (current && current.vault_id === event.vault_id ? { ...current, tree: event.tree, recent: event.recent } : current));
      registry.remoteChanged(event.changed);
      const current = selectedRef.current;
      if (current) {
        // 别处改了名、移了位置：跟着新路径走（同一个 file_id）。
        const moved = findFile(event.tree, current.file_id);
        if (moved && moved.path !== current.path) setSelected(moved);
        if (event.changed.includes("*") || event.changed.includes(current.file_id)) setRevision((n) => n + 1);
      }
    });
  }, [client]);

  // 页面隐藏 / 关闭时立即保存；有没存上的改动时拦住关闭（TDD-17 §5.2）；联网后立刻重发。
  useEffect(() => {
    const flush = () => void registry.flushAll();
    const onVisibility = () => {
      if (document.visibilityState === "hidden") flush();
    };
    const onBeforeUnload = (event: BeforeUnloadEvent) => {
      if (!registry.hasUnsaved()) return;
      event.preventDefault();
      event.returnValue = "";
    };
    const onOnline = () => registry.retryAll();
    document.addEventListener("visibilitychange", onVisibility);
    window.addEventListener("pagehide", flush);
    window.addEventListener("beforeunload", onBeforeUnload);
    window.addEventListener("online", onOnline);
    return () => {
      document.removeEventListener("visibilitychange", onVisibility);
      window.removeEventListener("pagehide", flush);
      window.removeEventListener("beforeunload", onBeforeUnload);
      window.removeEventListener("online", onOnline);
    };
  }, [registry]);

  // 「附件已上传」停一会儿就收起；失败与取消留着，直到下一次上传。
  useEffect(() => {
    if (upload?.kind !== "done") return;
    const timer = window.setTimeout(host.uploads.dismiss, UPLOAD_DONE_MS);
    return () => window.clearTimeout(timer);
  }, [host, upload]);

  function uploadText(status: UploadStatus): string {
    switch (status.kind) {
      case "uploading":
        return t("upload.uploading");
      case "done":
        return t("upload.done");
      case "cancelled":
        return t("upload.cancelled");
      case "save_first":
        return t("upload.saveFirst");
      case "failed":
        return t("upload.failed", { reason: saveErrorText(t, status.code, status.param, i18n.language) });
    }
  }

  /** 从「最近删除」恢复了一个文件：打开它。 */
  async function restored(file: WrittenFile) {
    const summary = await client.call("file_by_path", { path: file.path }).catch(() => null);
    if (summary) {
      setHistoryOf(null);
      setSelected(summary);
    }
  }

  async function openPath(path: string) {
    const file = await client.call("file_by_path", { path }).catch(() => null);
    if (file) setSelected(file);
  }

  function fail(error: unknown) {
    const { code, param } = codeOf(error);
    setNotice(saveErrorText(t, code, param, i18n.language));
  }

  /** 新建（PRD-21-022）：默认路径与桌面同一套命名，交给 Worker。 */
  function newNote(dir: string) {
    return createNote(dir, "");
  }

  async function createNote(dir: string, text: string) {
    setNotice(null);
    try {
      const path = await client.call("new_note_path", { dir, text, fallback_prefix: t("note.untitled") });
      const created = await client.call("create", { path, text });
      const summary = await client.call("file_by_path", { path: created.path });
      setSelected(summary ?? { file_id: created.file_id, path: created.path, name: created.path, size: 0, mtime_ms: null });
      setView("notes");
    } catch (error) {
      fail(error);
    }
  }

  /** 新建文件夹：空文件夹不同步，所以在里面新建第一篇笔记时它才出现（PRD-21-011）。 */
  async function newFolder(dir: string) {
    setNotice(null);
    const name = await dialogs.prompt({
      title: t("note.folder.title"),
      description: t("note.folder.hint"),
      label: t("note.folder.label"),
      value: "",
      confirmLabel: t("note.folder.confirm"),
    });
    if (name === null) return;
    try {
      await newNote(await client.call("child_path", { dir, name }));
    } catch (error) {
      fail(error);
    }
  }

  /** 改名、移动 = 同一个 `file_id` 换路径（PRD-21-022）；新路径的规则在 WASM。 */
  async function relocate(file: FileSummary, path: string) {
    try {
      await registry.get(file.file_id)?.flush();
      const written = await client.call("rename", { file_id: file.file_id, path });
      const summary = await client.call("file_by_path", { path: written.path });
      setSelected(summary ?? { ...file, path: written.path });
    } catch (error) {
      fail(error);
    }
  }

  async function rename(file: FileSummary) {
    setNotice(null);
    const name = await dialogs.prompt({
      title: t("note.rename.title"),
      label: t("note.rename.label"),
      value: file.name,
      confirmLabel: t("note.rename.confirm"),
    });
    if (name === null) return;
    try {
      await relocate(file, await client.call("renamed_path", { path: file.path, name }));
    } catch (error) {
      fail(error);
    }
  }

  async function move(file: FileSummary) {
    setNotice(null);
    const folders = foldersOf(opened?.tree ?? []);
    const dir = await dialogs.prompt({
      title: t("note.move.title"),
      label: t("note.move.label"),
      value: "",
      confirmLabel: t("note.move.confirm"),
      choices: [{ value: "", label: t("note.move.root") }, ...folders.map((folder) => ({ value: folder, label: folder }))],
    });
    if (dir === null) return;
    try {
      await relocate(file, await client.call("moved_path", { path: file.path, dir }));
    } catch (error) {
      fail(error);
    }
  }

  /** 删除（PRD-21-023）：写明会进各设备的 Lunarca 回收站；别处刚改过就不删并说明原因。 */
  async function remove(file: FileSummary, noteSession: NoteSession | null) {
    setNotice(null);
    const confirmed = await dialogs.confirm({
      title: t("note.delete.title", { name: file.name }),
      description: t("note.delete.body"),
      confirmLabel: t("note.delete.confirm"),
    });
    if (!confirmed) return;
    try {
      const rev = noteSession?.baseRev ?? (await client.call("head", { file_id: file.file_id }))?.rev;
      if (rev === undefined) return;
      const result = await client.call("delete", { file_id: file.file_id, rev });
      if (result.kind === "changed_elsewhere") {
        setNotice(t("note.delete.changedElsewhere"));
        return;
      }
      registry.discard(file.file_id);
      if (selectedRef.current?.file_id === file.file_id) setSelected(null);
    } catch (error) {
      fail(error);
    }
  }

  async function signOut() {
    await registry.flushAll();
    if (registry.hasUnsaved()) {
      const confirmed = await dialogs.confirm({
        title: t("note.signOutUnsaved.title"),
        description: t("note.signOutUnsaved.body", { count: registry.unsavedDrafts().length }),
        confirmLabel: t("note.signOutUnsaved.confirm"),
      });
      if (!confirmed) return;
    }
    onSignOut();
  }

  const vaultName = opened?.name ?? null;
  const isNote = selected !== null && classifyAssetTarget(selected.path) === null && !notText.has(selected.file_id);

  return (
    <div className="luna-web-shell" data-note-open={selected ? "true" : undefined}>
      <header className="luna-web-header">
        <span className="luna-web-header__brand">{t("app.name")}</span>
        {opened ? (
          <button
            type="button"
            className="luna-web-button luna-web-button--quiet luna-web-header__vault"
            onClick={() => {
              void registry.flushAll();
              setOpened(null);
              setSelected(null);
              setView("notes");
            }}
            title={t("vaults.switch")}
          >
            {vaultName ?? t("vaults.unnamed")}
          </button>
        ) : null}
        <div className="luna-web-header__end">
          <span className="luna-web-header__account" title={session.account.email}>
            {session.account.email}
          </span>
          <button type="button" className="luna-web-button luna-web-button--quiet" onClick={() => setView("account")}>
            {t("header.account")}
          </button>
          <button type="button" className="luna-web-button luna-web-button--quiet" onClick={() => void signOut()}>
            {t("header.signOut")}
          </button>
        </div>
      </header>
      {view === "account" ? (
        <main className="luna-web-main">
          <AccountPage
            client={client}
            session={session}
            idleSetting={idleSetting}
            onIdleSettingChange={setIdleSetting}
            onBack={() => setView("notes")}
            onSignOut={onSignOut}
          />
        </main>
      ) : opened ? (
        <div className="luna-web-body">
          <Sidebar
            client={client}
            tree={opened.tree}
            recent={opened.recent}
            selectedId={selected?.file_id ?? null}
            selectedPath={selected?.path ?? null}
            onSelect={(file) => setSelected(file)}
            canCreate={writeGate === null}
            onNewNote={(dir) => void newNote(dir)}
            onNewFolder={(dir) => void newFolder(dir)}
            onRestored={(file) => void restored(file)}
          />
          <main className="luna-web-main luna-web-reader">
            {notice ? (
              <p className="luna-web-error" role="alert" data-testid="workspace-notice">
                {notice}
              </p>
            ) : null}
            {selected ? (
              <>
                <div className="luna-web-reader__toolbar">
                  <button type="button" className="luna-web-button luna-web-button--quiet luna-web-reader__back" onClick={() => setSelected(null)}>
                    {t("reader.back")}
                  </button>
                  {historyOf !== selected.file_id ? (
                    <button type="button" className="luna-web-button luna-web-button--quiet" onClick={() => setHistoryOf(selected.file_id)}>
                      {t("history.open")}
                    </button>
                  ) : null}
                </div>
                {upload ? (
                  <p className="luna-web-hint" role="status" data-testid="upload-status" data-status={upload.kind}>
                    {uploadText(upload)}
                  </p>
                ) : null}
                {historyOf === selected.file_id ? (
                  <HistoryPanel
                    client={client}
                    file={selected}
                    readOnly={writeGate}
                    beforeRestore={async () => {
                      await registry.get(selected.file_id)?.flush();
                    }}
                    onBack={() => setHistoryOf(null)}
                    onOpenPath={(path) => void openPath(path)}
                    onRestored={() => setRevision((n) => n + 1)}
                  />
                ) : isNote ? (
                  <NotePanes
                    client={client}
                    host={host}
                    registry={registry}
                    file={selected}
                    mode={mode}
                    onModeChange={setMode}
                    readOnly={writeGate}
                    onOpenPath={(path) => void openPath(path)}
                    onNotText={(fileId) => setNotText((known) => new Set(known).add(fileId))}
                    onRename={(file) => void rename(file)}
                    onMove={(file) => void move(file)}
                    onDelete={(file, noteSession) => void remove(file, noteSession)}
                  />
                ) : (
                  <NoteReader client={client} file={selected} revision={revision} />
                )}
              </>
            ) : (
              <p className="luna-web-hint">{t("reader.empty")}</p>
            )}
          </main>
        </div>
      ) : (
        <main className="luna-web-main">
          {openProblem ? (
            <p className="luna-web-error" role="alert" data-testid="vault-open-error">
              {t(`error.${openProblem}`, { defaultValue: t("error.generic", { code: openProblem }) })}
            </p>
          ) : null}
          <VaultPicker onCreate={() => void createVault()} state={vaults} onOpen={(vault) => void openVault(vault)} onRetry={() => void loadVaults()} />
        </main>
      )}
      <Footer />
    </div>
  );
}
