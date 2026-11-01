/**
 * 改名 / 移动的回执条（PRD-06-030 ③）：「已更新 M 篇笔记中的 N 处链接」+「撤销」。
 *
 * 撤销凭据由 Rust 给、原样交回（同 ADR-135 标签撤销）；之后有任何新写入 Rust 就整次拒绝，
 * 界面只把原因说出来。撤销同样会改写别的笔记，故也先过保存屏障。条子挂一会儿就走——
 * 可逆动作值这几秒的安全感，但它不该赖着不走（与速记转正撤销条同一时长）。
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type RelocationReceiptDto } from "../../ipc/bindings";
import { RELOCATION_RECEIPT_EVENT } from "../../lib/app-events";
import { vaultErrorText } from "../../lib/vault-error";
import { afterEditorsSaved } from "../editor/editor-save-guard";

/** 回执条挂多久（毫秒）。 */
export const RELOCATION_RECEIPT_WINDOW_MS = 8_000;

/** 最近一次改名 / 移动的回执；过时自动收起。 */
export function useRelocationReceipt(): [RelocationReceiptDto | null, () => void] {
  const [receipt, setReceipt] = useState<RelocationReceiptDto | null>(null);
  useEffect(() => {
    const onReceipt = (event: Event) => {
      setReceipt((event as CustomEvent<RelocationReceiptDto>).detail);
    };
    window.addEventListener(RELOCATION_RECEIPT_EVENT, onReceipt);
    return () => window.removeEventListener(RELOCATION_RECEIPT_EVENT, onReceipt);
  }, []);
  useEffect(() => {
    if (receipt === null) {
      return;
    }
    const timer = window.setTimeout(() => setReceipt(null), RELOCATION_RECEIPT_WINDOW_MS);
    return () => window.clearTimeout(timer);
  }, [receipt]);
  const dismiss = useCallback(() => setReceipt(null), []);
  return [receipt, dismiss];
}

export function RelocationReceiptBar({
  receipt,
  onDismiss,
  onMovedBack,
  onStatus,
}: {
  receipt: RelocationReceiptDto;
  onDismiss: () => void;
  /** 撤销把某项搬回原处（新 → 旧）：外层据此映射标签页与选中项。 */
  onMovedBack: (from: string, to: string) => void;
  onStatus: (message: string) => void;
}) {
  const { t } = useTranslation("workspace");
  const { t: tCommon } = useTranslation("common");
  const [busy, setBusy] = useState(false);
  const summary =
    receipt.links > 0
      ? t("relocation.updated", { count: receipt.links, notes: receipt.notes })
      : t("relocation.noLinks");

  async function undo() {
    setBusy(true);
    const result = await afterEditorsSaved(
      () => commands.vaultUndoRelocation(receipt.undo),
      () => ({ status: "error" as const, error: tCommon("saveBlocksMove") }),
    );
    setBusy(false);
    onDismiss();
    if (result.status === "error") {
      onStatus(vaultErrorText(result.error));
      return;
    }
    for (const moved of result.data.moved_back) {
      onMovedBack(moved.from, moved.to);
    }
    onStatus(t("relocation.undone"));
  }

  return (
    <div className="luna-relocation-receipt" role="status">
      <span>{summary}</span>
      <button
        type="button"
        className="luna-btn luna-btn--secondary luna-btn--sm"
        disabled={busy}
        onClick={() => void undo()}
      >
        {t("relocation.undo")}
      </button>
    </div>
  );
}
