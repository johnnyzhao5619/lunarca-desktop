/**
 * 网页编辑器里的拖入与粘贴上传（PRD-21-025，TDD-17 §5.5）：剪贴板里的图片 / 文件与拖进正文的文件交给 Worker
 * 上传（分块、加密、提交为新文件），再在原处插入链接。附件放在哪、叫什么、链接怎么写由 WASM 按桌面「粘贴附件」
 * 的同一条规则给出；插入的 Markdown 形状（图片 `![](…)`、其它 `[名](…)`）用共享包的同一个函数。
 *
 * 插入点在上传期间冻结住（`paste-range`，与桌面同一套）：等上传的这会儿继续打字，链接仍落在原处；换了笔记就不插，
 * 并把刚上传的那份删掉（同桌面「插入被取消就回收附件」）。
 */

import type { Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { cancelPaste, freezePaste, insertFrozenPaste, pasteRangeField } from "@lunarca/editor-core/paste-range";
import { assetMarkdown, isImageAssetPath, pickFileItems } from "@lunarca/editor-core/paste-text";
import type { CommandMap } from "../../worker/protocol";
import { WorkerError } from "../../worker/worker-client";

/** 一次上传的进展（工作区据此写明正在上传、传好了、为什么没传上）。 */
export type UploadStatus =
  | { kind: "uploading" }
  | { kind: "done" }
  | { kind: "failed"; code: string; param?: number }
  | { kind: "cancelled" }
  | { kind: "save_first" };

export interface UploadDeps {
  call<C extends "upload" | "delete">(cmd: C, args: CommandMap[C]["args"]): Promise<CommandMap[C]["result"]>;
  report(status: UploadStatus): void;
}

export interface UploadTarget {
  /** 此刻打开的那一篇（链接相对于它）；没有为 `null`。 */
  notePath(): string | null;
  /** 写入闸的原因码；`null` = 可写。 */
  readOnly(): string | null;
}

type Kind = CommandMap["upload"]["args"]["kind"];

function codeOf(error: unknown): { code: string; param?: number } {
  return error instanceof WorkerError ? { code: error.code, param: error.param } : { code: "internal" };
}

async function uploadAll(
  view: EditorView,
  deps: UploadDeps,
  target: UploadTarget,
  files: readonly File[],
  kind: Kind,
  range: { from: number; to: number },
): Promise<void> {
  const gate = target.readOnly();
  if (gate !== null) {
    deps.report({ kind: "failed", code: gate });
    return;
  }
  const notePath = target.notePath();
  if (!notePath) {
    deps.report({ kind: "save_first" });
    return;
  }
  const id = freezePaste(view, range);
  deps.report({ kind: "uploading" });
  const inserts: string[] = [];
  const uploaded: CommandMap["upload"]["result"][] = [];
  let failure: { code: string; param?: number } | null = null;
  for (const file of files) {
    try {
      const done = await deps.call("upload", { note_path: notePath, file, name: file.name, mime: file.type, kind });
      uploaded.push(done);
      // 图片与否：粘贴物按 MIME，拖入的文件按落盘名的扩展名（与桌面两条路各自的判据相同）。
      inserts.push(assetMarkdown(done.link, kind === "pasted" ? file.type.startsWith("image/") : isImageAssetPath(done.link)));
    } catch (error) {
      failure = codeOf(error);
      break;
    }
  }
  if (inserts.length === 0) {
    cancelPaste(view, id);
    deps.report(failure ? { kind: "failed", ...failure } : { kind: "done" });
    return;
  }
  if (target.notePath() !== notePath || !insertFrozenPaste(view, id, inserts.join("\n"), true)) {
    cancelPaste(view, id);
    for (const file of uploaded) void deps.call("delete", { file_id: file.file_id, rev: file.rev }).catch(() => {});
    deps.report({ kind: "cancelled" });
    return;
  }
  deps.report(failure ? { kind: "failed", ...failure } : { kind: "done" });
}

/**
 * 工具条与斜杠菜单「插入附件」选中的文件（PRD-21-025）：与拖入同一条上传路（落点、命名、上限预检与进度
 * 都在 Worker / WASM），返回落好的链接，由编辑器的附件动作插进正文。只读或还没存成文件时不传、返回空。
 */
export async function uploadPicked(deps: UploadDeps, target: UploadTarget, files: readonly File[]): Promise<string[]> {
  const gate = target.readOnly();
  if (gate !== null) {
    deps.report({ kind: "failed", code: gate });
    return [];
  }
  const notePath = target.notePath();
  if (!notePath) {
    deps.report({ kind: "save_first" });
    return [];
  }
  deps.report({ kind: "uploading" });
  const links: string[] = [];
  for (const file of files) {
    try {
      const done = await deps.call("upload", { note_path: notePath, file, name: file.name, mime: file.type, kind: "named" });
      links.push(done.link);
    } catch (error) {
      deps.report({ kind: "failed", ...codeOf(error) });
      return links;
    }
  }
  deps.report({ kind: "done" });
  return links;
}

/** 编辑器扩展：粘贴文件、拖入文件即上传并插入链接；纯文本粘贴与别的拖放照常交给编辑器。 */
export function uploadExtension(deps: UploadDeps, target: UploadTarget): Extension {
  return [
    pasteRangeField,
    EditorView.domEventHandlers({
      paste(event, view) {
        const data = event.clipboardData;
        if (!data) return false;
        const files = pickFileItems(data.items)
          .map((item) => item.getAsFile())
          .filter((file): file is File => file !== null);
        if (files.length === 0) return false;
        event.preventDefault();
        void uploadAll(view, deps, target, files, "pasted", view.state.selection.main);
        return true;
      },
      dragover(event) {
        if (!event.dataTransfer?.types.includes("Files")) return false;
        event.preventDefault();
        return true;
      },
      drop(event, view) {
        const files = [...(event.dataTransfer?.files ?? [])];
        if (files.length === 0) return false;
        event.preventDefault();
        const at = view.posAtCoords({ x: event.clientX, y: event.clientY }) ?? view.state.selection.main.head;
        void uploadAll(view, deps, target, files, "named", { from: at, to: at });
        return true;
      },
    }),
  ];
}

/** 上传进展的一份共享状态：编辑器写、工作区读（订阅 + 快照，给 `useSyncExternalStore`）。 */
export class UploadReporter {
  private status: UploadStatus | null = null;
  private readonly listeners = new Set<() => void>();

  readonly report = (status: UploadStatus | null): void => {
    this.status = status;
    for (const listener of this.listeners) listener();
  };

  readonly dismiss = (): void => this.report(null);

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  readonly snapshot = (): UploadStatus | null => this.status;
}
