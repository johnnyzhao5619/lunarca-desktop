/**
 * 冲突副本横幅（TDD-06 §4.1「同步盘痕迹」①，笔记库对标轮 V3b）。
 *
 * 两台设备同时改了同一篇，同步盘不合并、多出一份副本；git 合并冲突把两边连同 `<<<<<<<`
 * 标记一起写进正文。打开这样一篇时在页头下方说清楚，并给「对比」与「保留哪份」——
 * **不自动合并、不自动删除**。判定全在 Rust（ADR-003）；保留哪份会改写原文、移走副本，
 * 故先确认（说清删除后果）、再过保存屏障。
 *
 * 对比复用外部修改冲突同一份差异视图（`noteEditDiff` + `DiffView`）。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { DiffView } from "../../components/ui/DiffView";
import { useConfirm } from "../../components/ConfirmHost";
import { commands, type ConflictCopyDto, type LineDiffDto } from "../../ipc/bindings";
import { vaultErrorText } from "../../lib/vault-error";
import { useDeleteConsequence } from "../../hooks/use-delete-mode";
import { afterEditorsSaved } from "./editor-save-guard";

function noteName(path: string): string {
  return (path.split("/").pop() ?? path).replace(/\.md$/i, "");
}

export function ConflictCopyBanner({
  path,
  currentText,
  onOpenPath,
  onStatus,
}: {
  path: string;
  /** 编辑器里此刻的正文（对比用这一份，未保存的改动也算）。 */
  currentText: () => string;
  onOpenPath?: (path: string) => void;
  onStatus: (message: string) => void;
}) {
  const { t } = useTranslation("editor");
  const { t: tCommon } = useTranslation("common");
  const ask = useConfirm();
  const consequence = useDeleteConsequence();
  const [copy, setCopy] = useState<ConflictCopyDto | null>(null);
  const [diff, setDiff] = useState<LineDiffDto | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let active = true;
    setCopy(null);
    setDiff(null);
    void commands.conflictCopyInfo(path).then((result) => {
      if (active && result.status === "ok") setCopy(result.data);
    });
    return () => {
      active = false;
    };
  }, [path]);

  if (!copy) return null;

  if (copy.kind === "git_markers" || !copy.original) {
    return (
      <div className="luna-editor__conflict" role="status">
        <span>{t("conflictCopy.gitMarkers")}</span>
      </div>
    );
  }
  const original = copy.original;

  async function compare() {
    const read = await commands.readNote(original);
    if (read.status !== "ok") {
      onStatus(vaultErrorText(read.error));
      return;
    }
    const result = await commands.noteEditDiff(read.data.content, currentText());
    if (result.status === "ok") setDiff(result.data);
    else onStatus(t("error"));
  }

  async function resolve(keepCopy: boolean) {
    const ok = await ask({
      title: keepCopy
        ? t("conflictCopy.keepCopyTitle", { name: noteName(original) })
        : t("conflictCopy.keepOriginalTitle"),
      description: keepCopy
        ? t("conflictCopy.keepCopyDesc", { consequence })
        : t("conflictCopy.keepOriginalDesc", { consequence }),
      confirmLabel: keepCopy ? t("conflictCopy.keepCopy") : t("conflictCopy.keepOriginal"),
      destructive: true,
    });
    if (!ok) return;
    setBusy(true);
    const result = await afterEditorsSaved(
      () => commands.resolveConflictCopy(path, keepCopy),
      () => ({ status: "error" as const, error: tCommon("saveBlocksMove") }),
    );
    setBusy(false);
    if (result.status === "error") {
      onStatus(vaultErrorText(result.error));
      return;
    }
    onStatus(keepCopy ? t("conflictCopy.keptCopy") : t("conflictCopy.keptOriginal"));
    onOpenPath?.(result.data);
  }

  return (
    <>
      <div className="luna-editor__conflict" role="status" title={original}>
        <span>
          {t("conflictCopy.copyOf", {
            name: noteName(original),
            source: t(`conflictCopy.source.${copy.kind}`),
          })}
        </span>
        <button type="button" disabled={busy} onClick={() => void compare()}>
          {t("conflictCopy.compare")}
        </button>
        <button type="button" disabled={busy} onClick={() => void resolve(true)}>
          {t("conflictCopy.keepCopy")}
        </button>
        <button type="button" disabled={busy} onClick={() => void resolve(false)}>
          {t("conflictCopy.keepOriginal")}
        </button>
      </div>
      {diff ? (
        <div className="luna-editor__conflict-diff">
          <DiffView diff={diff} />
        </div>
      ) : null}
    </>
  );
}
