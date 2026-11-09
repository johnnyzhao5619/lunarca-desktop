/**
 * 网页上打开的一篇笔记（TDD-17 §5.1–§5.3、PRD-21-021 / 030 / 031 / 032）：编辑器状态、它所基于的版本、
 * 自动保存的节拍与四种保存状态、别处改了之后怎么办。合并、冲突副本、命名与幂等重发都在 Worker / WASM 里，
 * 这里只编排「什么时候存、存完怎么把结果落回编辑器」。
 *
 * 编辑器挂着时状态在 `EditorView` 里，换走（切到别的笔记）时收回成一份 `EditorState` 留在这里：没存上的
 * 改动、撤销历史与光标都跟着它走，切回来原样接上；断网时换篇也不丢。交给 Worker 写的正文只从
 * `editorDocumentText` 出来（PRD-21-020 逐字节一致）。
 */

import { ChangeSet, Transaction, type ChangeSpec, type EditorState } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { changeOverEdits, editorDocumentText, minimalChange } from "@lunarca/editor-core/save-session";
import type { CommandMap, CommandName, SaveResult, WrittenFile } from "../../worker/protocol";
import { WorkerError } from "../../worker/worker-client";

/** 停手多久保存（TDD-17 §5.2）。 */
export const SAVE_IDLE_MS = 1_000;
/** 连续输入时最长多久强制保存一次。 */
export const SAVE_MAX_MS = 10_000;
/** 断网或服务端暂时不可用时的重试退避（联网事件另会立刻触发一次）。 */
export const RETRY_DELAYS_MS = [2_000, 5_000, 10_000, 30_000];

/** 界面上只有这四种保存状态（PRD-21-021）。 */
export type SaveStatus =
  | { kind: "saved" }
  | { kind: "saving" }
  | { kind: "offline" }
  | { kind: "failed"; code: string; param?: number };

/** 「已合并别处的改动」那条提示（可撤回）。 */
export interface MergeNotice {
  id: number;
  /** 撤回要以哪一版为基线、换回什么（Worker 给的 `rev` / `undo_text`）。 */
  rev: number;
  undoText: string;
  /** 合并结果：编辑器此刻还是它，撤回才有意义（之后又打了字就不再提供撤回）。 */
  mergedText: string;
}

export interface SessionSnapshot {
  status: SaveStatus;
  /** 别处改了这篇、而这边还有没存上的改动（横幅「别处改了这篇」）：别处那一版的 rev。 */
  remoteRev: number | null;
  /** 这篇在别处被删了（清单里没有它了）。 */
  gone: boolean;
  merged: MergeNotice | null;
  /** 两边改了同一段：本方另存成的冲突副本（主文件此刻是别处的版本）。 */
  conflictCopy: WrittenFile | null;
}

/** 断网：Worker 留着待存的那份，联网后用同一个幂等键重发。 */
const OFFLINE = "network_unavailable";
/** 过一会儿再试可能就成的失败（服务端忙、限流、暂时不可用）：照样自动重试，状态写「保存失败」带原因。 */
const RETRYABLE = /^(?:rate_limited|sync_busy|storage_unavailable|service_unavailable|internal|http_5\d\d)$/;

type Call = <C extends CommandName>(cmd: C, args: CommandMap[C]["args"]) => Promise<CommandMap[C]["result"]>;

export interface SessionDeps {
  call: Call;
  /** 冲突副本名里的说明（本地化的「冲突 网页」，ADR-148 ⑥）。 */
  conflictLabel(): string;
  /** 写入闸的原因码（只读时不发保存）。 */
  readOnly(): string | null;
  setTimeout(run: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

function codeOf(error: unknown): { code: string; param?: number } {
  return error instanceof WorkerError ? { code: error.code, param: error.param } : { code: "internal" };
}

export class NoteSession {
  readonly fileId: string;
  path: string;
  /** 编辑器内容所基于的版本。 */
  baseRev: number;
  /** 那一版的正文（存上的或载入的）：编辑器与它不同即「有没存上的改动」。 */
  private savedText: string;
  private state: EditorState;
  private view: EditorView | null = null;
  private snapshot: SessionSnapshot = { status: { kind: "saved" }, remoteRev: null, gone: false, merged: null, conflictCopy: null };
  private readonly listeners = new Set<() => void>();
  private readonly deps: SessionDeps;

  private idleTimer: unknown = null;
  private maxTimer: unknown = null;
  private retryTimer: unknown = null;
  private retryAttempt = 0;
  /** 在途的那一次保存；自它开始以来编辑器里的改动记在 `flightEdits`。 */
  private flight: Promise<void> | null = null;
  private flightEdits: ChangeSet | null = null;
  /** 在途期间又有人要存（或别处又改了）：落定之后再来一轮。 */
  private again = false;
  private remoteCheckPending = false;
  private nextNoticeId = 1;
  private disposed = false;
  /** 从封存里恢复的：状态里还没有编辑器的扩展，挂上之前要换成带扩展的那份。 */
  restored = false;

  constructor(deps: SessionDeps, file: { file_id: string; path: string; rev: number; text: string }, state: EditorState) {
    this.deps = deps;
    this.fileId = file.file_id;
    this.path = file.path;
    this.baseRev = file.rev;
    this.savedText = file.text;
    this.state = state;
  }

  // ── 订阅（React 经 useSyncExternalStore 读） ────────────────────────────────

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getSnapshot = (): SessionSnapshot => this.snapshot;

  private update(patch: Partial<SessionSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...patch };
    for (const listener of this.listeners) listener();
  }

  /** 恢复来的这一篇第一次上编辑器：换成带扩展的状态，正文仍是自己的。 */
  adopt(state: EditorState): void {
    this.state = state.update({
      changes: { from: 0, to: state.doc.length, insert: this.text() },
      annotations: Transaction.addToHistory.of(false),
    }).state;
    this.restored = false;
  }

  /** 这一篇的基线（封存用）。 */
  get savedBaseline(): string {
    return this.savedText;
  }

  // ── 编辑器的挂接 ───────────────────────────────────────────────────────────

  /** 挂到一个编辑器上：编辑器换成这一篇的状态（正文、撤销历史、光标都是它自己的）。 */
  attach(view: EditorView): void {
    this.view = view;
    view.setState(this.state);
  }

  /** 从编辑器上取下：状态收回来留着（没存上的改动不丢）。 */
  detach(): void {
    if (!this.view) return;
    this.state = this.view.state;
    this.view = null;
  }

  get attached(): boolean {
    return this.view !== null;
  }

  private current(): EditorState {
    return this.view ? this.view.state : this.state;
  }

  /** 这一刻要交给 Worker 写的正文（唯一出口 `editorDocumentText`）。 */
  text(): string {
    return editorDocumentText(this.current());
  }

  /** 有没有没存上的改动（含在途的那一次）。 */
  get unsaved(): boolean {
    return this.flight !== null || this.text() !== this.savedText;
  }

  /** 把一次「从外面来的」改动落到编辑器上：不进撤销历史，光标与滚动随改动映射。 */
  private apply(changes: ChangeSpec): void {
    const spec = { changes, annotations: [Transaction.addToHistory.of(false)] };
    if (this.view) this.view.dispatch(spec);
    else this.state = this.state.update(spec).state;
  }

  /** 编辑器每一次文档变化（挂着时由编辑器的更新监听调）。 */
  docChanged(changes: ChangeSet, userEdit: boolean): void {
    if (this.flightEdits) this.flightEdits = this.flightEdits.compose(changes);
    if (!userEdit) return;
    // 打了字：「已合并」的撤回不再成立（撤回只换回合并那一刻的样子）。
    if (this.snapshot.merged && this.text() !== this.snapshot.merged.mergedText) this.update({ merged: null });
    this.schedule();
  }

  // ── 自动保存的节拍 ─────────────────────────────────────────────────────────

  /** 停手 1 秒保存；连续输入时自第一下起最长 10 秒强制保存一次。 */
  private schedule(): void {
    if (this.disposed) return;
    if (this.snapshot.status.kind === "saved") this.update({ status: { kind: "saving" } });
    if (this.idleTimer !== null) this.deps.clearTimeout(this.idleTimer);
    this.idleTimer = this.deps.setTimeout(() => {
      this.idleTimer = null;
      void this.flush();
    }, SAVE_IDLE_MS);
    if (this.maxTimer === null) {
      this.maxTimer = this.deps.setTimeout(() => {
        this.maxTimer = null;
        void this.flush();
      }, SAVE_MAX_MS);
    }
  }

  private clearTimers(): void {
    for (const handle of [this.idleTimer, this.maxTimer, this.retryTimer]) if (handle !== null) this.deps.clearTimeout(handle);
    this.idleTimer = null;
    this.maxTimer = null;
    this.retryTimer = null;
  }

  private scheduleRetry(): void {
    if (this.retryTimer !== null || this.disposed) return;
    const delay = RETRY_DELAYS_MS[Math.min(this.retryAttempt, RETRY_DELAYS_MS.length - 1)];
    this.retryAttempt += 1;
    this.retryTimer = this.deps.setTimeout(() => {
      this.retryTimer = null;
      void this.flush();
    }, delay);
  }

  /**
   * 立即保存（失焦、切换笔记、页面隐藏、`pagehide`、联网、计时到点）。同一篇在途时不再并发发一份：
   * 记下「还要再存」，落定之后以新的基线再来一轮（Worker 那边同样串行）。
   */
  flush(): Promise<void> {
    if (this.flight) {
      this.again = true;
      return this.flight;
    }
    if (this.disposed || this.text() === this.savedText) {
      this.clearTimers();
      if (this.snapshot.status.kind === "saving") this.update({ status: { kind: "saved" } });
      return Promise.resolve();
    }
    const gate = this.deps.readOnly();
    if (gate !== null) {
      this.clearTimers();
      this.update({ status: { kind: "failed", code: gate } });
      return Promise.resolve();
    }
    this.clearTimers();
    const text = this.text();
    const base = this.baseRev;
    this.flightEdits = ChangeSet.empty(this.current().doc.length);
    if (this.snapshot.status.kind === "saved") this.update({ status: { kind: "saving" } });
    this.flight = this.deps
      .call("save", { file_id: this.fileId, base_rev: base, text, conflict_label: this.deps.conflictLabel() })
      .then(
        (result) => this.settle(text, result),
        (error: unknown) => this.fail(error),
      )
      .finally(() => {
        this.flight = null;
        this.flightEdits = null;
        const again = this.again;
        this.again = false;
        if (this.remoteCheckPending) {
          this.remoteCheckPending = false;
          void this.checkRemote();
        }
        if (again && this.snapshot.status.kind !== "failed" && this.snapshot.status.kind !== "offline") void this.flush();
        else if (!again && this.text() !== this.savedText && this.snapshot.status.kind === "saving") this.schedule();
      });
    return this.flight;
  }

  private settle(text: string, result: SaveResult): void {
    this.retryAttempt = 0;
    const edits = this.flightEdits ?? ChangeSet.empty(text.length);
    this.baseRev = result.rev;
    this.path = result.path;
    if (result.kind === "saved") {
      this.savedText = text;
    } else if (result.kind === "merged") {
      // 编辑器换成合并结果（最小差量），在途期间接着打的字叠在上面。撤回换回的是「没有别处那些改动」的本方
      // 全文：在途期间没再打字时就是 Worker 给的 `undo_text`。
      const mine = this.text();
      this.apply(changeOverEdits(text, result.text, edits));
      this.savedText = result.text;
      this.update({
        merged: { id: this.nextNoticeId++, rev: result.rev, undoText: edits.empty ? result.undo_text : mine, mergedText: this.text() },
      });
    } else {
      // 两边改了同一段：主文件换成别处的版本，本方这份已另存为冲突副本。
      const mine = this.text();
      if (result.text !== null) {
        this.apply(minimalChange(mine, result.text));
        this.savedText = result.text;
      }
      this.update({ conflictCopy: result.copy });
      if (mine !== text) {
        // 在途期间接着打的字也属于本方那一份：补进副本，不留在主文件里冒充别处的版本。
        void this.deps
          .call("save", { file_id: result.copy.file_id, base_rev: result.copy.rev, text: mine, conflict_label: this.deps.conflictLabel() })
          .catch(() => {});
      }
    }
    const pending = this.text() !== this.savedText;
    this.update({
      status: pending ? { kind: "saving" } : { kind: "saved" },
      remoteRev: this.snapshot.remoteRev !== null && this.snapshot.remoteRev <= this.baseRev ? null : this.snapshot.remoteRev,
      gone: false,
    });
  }

  private fail(error: unknown): void {
    const { code, param } = codeOf(error);
    if (code === OFFLINE) {
      this.update({ status: { kind: "offline" } });
      this.scheduleRetry();
      return;
    }
    this.update({ status: param === undefined ? { kind: "failed", code } : { kind: "failed", code, param } });
    if (RETRYABLE.test(code)) this.scheduleRetry();
  }

  /** 联网了：断网期间攒下的改动立刻用同一个幂等键重发（Worker 认回那一份）。 */
  retryNow(): void {
    if (this.retryTimer !== null) this.deps.clearTimeout(this.retryTimer);
    this.retryTimer = null;
    this.retryAttempt = 0;
    if (this.text() !== this.savedText) void this.flush();
  }

  // ── 别处改了（TDD-17 §5.1、PRD-21-030） ────────────────────────────────────

  /**
   * 清单说这篇变了：比一下清单里的 rev。没有没存上的改动 → 原地更新（最小差量，光标不动）；有 → 出横幅。
   * 保存在途时先不比：那多半就是本页自己的提交，落定之后再比。
   */
  async checkRemote(): Promise<void> {
    if (this.flight) {
      this.remoteCheckPending = true;
      return;
    }
    let head: CommandMap["head"]["result"];
    try {
      head = await this.deps.call("head", { file_id: this.fileId });
    } catch {
      return;
    }
    if (this.disposed) return;
    if (head === null) {
      this.update({ gone: true });
      return;
    }
    this.path = head.path;
    if (head.rev <= this.baseRev) return;
    if (this.text() !== this.savedText || this.flight) {
      this.update({ remoteRev: head.rev });
      return;
    }
    let note: CommandMap["read"]["result"];
    try {
      note = await this.deps.call("read", { file_id: this.fileId });
    } catch {
      return;
    }
    if (this.disposed || note.rev <= this.baseRev) return;
    if (this.text() !== this.savedText || this.flight) {
      // 取正文的这一会儿里打了字：不覆盖，改出横幅。
      this.update({ remoteRev: note.rev });
      return;
    }
    this.apply(minimalChange(this.text(), note.text));
    this.savedText = note.text;
    this.baseRev = note.rev;
    this.path = note.path;
    this.update({ remoteRev: null });
  }

  /** 横幅「合并」：以载入时的版本为基线立刻保存——过期之后由 Worker 做三路合并（与桌面同一个合并器）。 */
  merge(): Promise<void> {
    return this.flush();
  }

  /** 横幅「另存我的这份」：本方内容另存为副本，这一篇换成别处的版本。 */
  async saveMineAsCopy(): Promise<WrittenFile> {
    const mine = this.text();
    const copy = await this.deps.call("save_copy", { file_id: this.fileId, text: mine, conflict_label: this.deps.conflictLabel() });
    const note = await this.deps.call("read", { file_id: this.fileId });
    this.apply(minimalChange(this.text(), note.text));
    this.savedText = note.text;
    this.baseRev = note.rev;
    this.path = note.path;
    this.clearTimers();
    this.update({ remoteRev: null, status: { kind: "saved" }, conflictCopy: copy });
    return copy;
  }

  /** 撤回「已合并别处的改动」：换回本方原文，以合并那一版为基线再存一次。 */
  undoMerge(): Promise<void> {
    const merged = this.snapshot.merged;
    if (!merged || this.text() !== merged.mergedText) return Promise.resolve();
    this.update({ merged: null });
    this.apply(minimalChange(this.text(), merged.undoText));
    return this.flush();
  }

  dismissMerged(): void {
    this.update({ merged: null });
  }

  dismissConflict(): void {
    this.update({ conflictCopy: null });
  }

  /** 写入闸变了（进入只读）：在途之外的改动留在本页，状态如实写原因。 */
  readOnlyChanged(): void {
    const gate = this.deps.readOnly();
    if (gate !== null && this.text() !== this.savedText) {
      this.clearTimers();
      this.update({ status: { kind: "failed", code: gate } });
    }
  }

  /** 不再需要这一篇（删了 / 退出）：停掉计时，之后不再发任何保存。 */
  dispose(): void {
    this.disposed = true;
    this.clearTimers();
    this.listeners.clear();
  }
}
