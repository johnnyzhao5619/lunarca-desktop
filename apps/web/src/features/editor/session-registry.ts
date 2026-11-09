/**
 * 本页打开过、还没存完的笔记（TDD-17 §5.2）：一篇一个 `NoteSession`，按 `file_id` 留着。编辑器换走之后
 * 还有没存上的改动的那几篇照样留着、照样重试；存完、又不在编辑器上的就放掉。页面隐藏 / `pagehide` /
 * 锁定前一次冲刷全部；`beforeunload` 与会话结束据此知道还有没有没存上的内容。
 */

import { EditorState } from "@codemirror/state";
import type { NoteText, SealedDraft } from "../../worker/protocol";
import { NoteSession, type SessionDeps } from "./note-session";

/** 会话结束时还没存上的一篇（给「先提示未保存的内容」用，PRD-21-004）。 */
export type UnsavedDraft = SealedDraft;

export class SessionRegistry {
  private readonly sessions = new Map<string, NoteSession>();
  private readonly deps: Omit<SessionDeps, "readOnly">;
  private gate: string | null = null;

  constructor(deps: Omit<SessionDeps, "readOnly">) {
    this.deps = deps;
  }

  get(fileId: string): NoteSession | undefined {
    return this.sessions.get(fileId);
  }

  open(note: NoteText, state: EditorState): NoteSession {
    const existing = this.sessions.get(note.file_id);
    if (existing) {
      if (existing.restored) existing.adopt(state);
      return existing;
    }
    const session = new NoteSession({ ...this.deps, readOnly: () => this.gate }, note, state);
    this.sessions.set(note.file_id, session);
    return session;
  }

  /** 解锁后放回封存的改动，接着存：存上了就放掉，这期间打开的话编辑器接着用它。 */
  restore(drafts: readonly SealedDraft[]): void {
    for (const draft of drafts) {
      const session = new NoteSession(
        { ...this.deps, readOnly: () => this.gate },
        { file_id: draft.file_id, path: draft.path, rev: draft.base_rev, text: draft.saved_text },
        EditorState.create({ doc: draft.text }),
      );
      session.restored = true;
      this.sessions.set(draft.file_id, session);
      this.release(draft.file_id);
    }
  }

  /** 编辑器换走了这一篇：存完了就放掉，没存完的留着继续存。 */
  release(fileId: string): void {
    const session = this.sessions.get(fileId);
    if (!session || session.attached) return;
    void session.flush().then(() => {
      if (!session.attached && !session.unsaved && this.sessions.get(fileId) === session) {
        session.dispose();
        this.sessions.delete(fileId);
      }
    });
  }

  /** 删掉了这一篇：没存上的改动随之作废，不再发任何保存。 */
  discard(fileId: string): void {
    this.sessions.get(fileId)?.dispose();
    this.sessions.delete(fileId);
  }

  flushAll(): Promise<void> {
    return Promise.all([...this.sessions.values()].map((session) => session.flush())).then(() => undefined);
  }

  retryAll(): void {
    for (const session of this.sessions.values()) session.retryNow();
  }

  /** 清单说这些文件变了（`*` = 全量重载）：各自比一下 rev。 */
  remoteChanged(changed: readonly string[]): void {
    const all = changed.includes("*");
    for (const session of this.sessions.values()) {
      if (all || changed.includes(session.fileId)) void session.checkRemote();
    }
  }

  get readOnly(): string | null {
    return this.gate;
  }

  setReadOnly(reason: string | null): void {
    this.gate = reason;
    for (const session of this.sessions.values()) session.readOnlyChanged();
  }

  hasUnsaved(): boolean {
    return [...this.sessions.values()].some((session) => session.unsaved);
  }

  unsavedDrafts(): UnsavedDraft[] {
    return [...this.sessions.values()]
      .filter((session) => session.unsaved)
      .map((session) => ({
        file_id: session.fileId,
        path: session.path,
        base_rev: session.baseRev,
        saved_text: session.savedBaseline,
        text: session.text(),
      }));
  }

  dispose(): void {
    for (const session of this.sessions.values()) session.dispose();
    this.sessions.clear();
  }
}
