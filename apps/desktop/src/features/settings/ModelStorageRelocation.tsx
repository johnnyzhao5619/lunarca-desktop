/**
 * 模型存储搬移（PRD-10-014 / TDD-04 §2.3.3）：把 Lunarca 受管模型根搬到另一目录或另一块盘。
 *
 * 流程全在 Rust（预检 → 拷贝 → 校验 → 切换 → 删旧，可取消、旧位置在切换前始终完好）；
 * 本组件只做四件事：选目录（原生对话框）→ 展示计划（字节、目标卷剩余、不能开始的原因）
 * → 确认后开始 → 渲染进度 / 取消 / 终态。不解析路径、不判空间（ADR-003）。
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useConfirm } from "../../components/ConfirmHost";
import { HelpDetails } from "../../components/ui/HelpDetails";
import { PathDetails } from "../../components/ui/PathDetails";
import {
  commands,
  events,
  type StorageRelocationPlanDto,
  type StorageRelocationProgress,
} from "../../ipc/bindings";
import { formatBytes } from "../../lib/format-bytes";
import { vaultErrorText } from "../../lib/vault-error";

const TERMINAL = new Set(["done", "failed", "cancelled"]);

/** 稳定码 → 文案子键；认不出的码原样显示（它是排查线索）。 */
const REASON_KEYS: Record<string, string> = {
  RELOCATION_SAME_DIR: "same_dir",
  RELOCATION_NESTED: "nested",
  RELOCATION_TARGET_NOT_DIR: "not_dir",
  RELOCATION_TARGET_NOT_EMPTY: "not_empty",
  RELOCATION_TARGET_NOT_WRITABLE: "not_writable",
  RELOCATION_TARGET_NOT_ABSOLUTE: "not_absolute",
  RELOCATION_INSUFFICIENT_DISK: "insufficient_disk",
  RELOCATION_INSTALL_IN_PROGRESS: "install_in_progress",
  RELOCATION_ALREADY_RUNNING: "already_running",
  RELOCATION_SOURCE_UNREADABLE: "source_unreadable",
};

export function ModelStorageRelocation({
  modelsRoot,
  onRelocated,
}: {
  modelsRoot: string;
  /** 搬完（切换已生效）后让上层重取模型清单与存储事实。 */
  onRelocated: () => void;
}) {
  const { t, i18n } = useTranslation("settings");
  const ask = useConfirm();
  const [progress, setProgress] = useState<StorageRelocationProgress | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // 回调进 ref：订阅只建一次，但终态时叫的永远是最新那一个（上层每次渲染都会给新函数）。
  const onRelocatedRef = useRef(onRelocated);
  onRelocatedRef.current = onRelocated;

  useEffect(() => {
    void commands.storageRelocationStatus().then((result) => {
      if (result.status === "ok" && result.data && !TERMINAL.has(result.data.phase)) {
        setProgress(result.data);
      }
    });
    const unlisten = events.modelsRelocation.listen((event) => {
      const next = event.payload;
      setProgress(TERMINAL.has(next.phase) ? null : next);
      if (next.phase === "done") {
        setNote(
          next.error
            ? t("models.relocate.done_old_kept", { path: next.to })
            : t("models.relocate.done", { path: next.to }),
        );
        onRelocatedRef.current();
      } else if (next.phase === "failed") {
        setNote(t("models.relocate.failed", { reason: reasonText(next.error ?? "") }));
      } else if (next.phase === "cancelled") {
        setNote(t("models.relocate.cancelled"));
      }
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [t]);

  function reasonText(code: string): string {
    const head = code.split(/[\s:(]/, 1)[0] ?? "";
    const sub = REASON_KEYS[head];
    return sub ? t(`models.relocate.reason.${sub}`) : vaultErrorText(code);
  }

  async function chooseAndPlan() {
    setNote(null);
    const picked = await commands.pickFolder(null);
    if (picked.status !== "ok" || picked.data === null) {
      return;
    }
    setBusy(true);
    let planned: Awaited<ReturnType<typeof commands.planStorageRelocation>>;
    try {
      planned = await commands.planStorageRelocation(picked.data);
    } finally {
      setBusy(false);
    }
    if (planned.status === "error") {
      setNote(reasonText(planned.error));
      return;
    }
    const plan: StorageRelocationPlanDto = planned.data;
    if (!plan.can_start) {
      setNote(reasonText(plan.blocking_reason ?? ""));
      return;
    }
    const ok = await ask({
      title: t("models.relocate.confirm_title"),
      description: t("models.relocate.confirm", {
        from: plan.from,
        to: plan.to,
        bytes: formatBytes(plan.bytes, i18n.language),
        available: formatBytes(plan.available_bytes, i18n.language),
      }),
      confirmLabel: t("models.relocate.confirm_action"),
      cancelLabel: t("models.relocate.cancel"),
      destructive: false,
    });
    if (!ok) {
      return;
    }
    const started = await commands.startStorageRelocation(plan.plan_id);
    if (started.status === "error") {
      setNote(reasonText(started.error));
    }
  }

  const total = progress?.total_bytes ?? 0;
  const copied = progress?.copied_bytes ?? 0;
  const ratio = progress && total > 0 ? Math.min(1, copied / total) : null;

  return (
    <div className="luna-settings-relocate">
      <div className="luna-settings-panel__actions">
        <span className="luna-settings-panel__section-title">{t("models.storage_root")}</span>
        <HelpDetails summary={t("models.storage_root")}>
          <p>{t("models.relocate.hint")}</p>
        </HelpDetails>
      </div>
      <PathDetails path={modelsRoot} />
      {progress ? (
        <div className="luna-settings-relocate__progress">
          <span className="luna-settings-panel__hint" role="status">
            {t(`models.relocate.phase.${progress.phase}`, {
              copied: formatBytes(copied, i18n.language),
              total: formatBytes(total, i18n.language),
            })}
          </span>
          <progress className="luna-progress" value={ratio ?? undefined} max={1} />
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-btn--sm"
            disabled={progress.phase === "switching" || progress.phase === "cleaning"}
            onClick={() => void commands.cancelStorageRelocation()}
          >
            {t("models.relocate.cancel")}
          </button>
        </div>
      ) : (
        <div className="luna-settings-panel__actions">
          {note ? <span className="luna-settings-panel__hint" role="status">{note}</span> : null}
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            disabled={busy}
            onClick={() => void chooseAndPlan()}
          >
            {t("models.relocate.action")}
          </button>
        </div>
      )}
    </div>
  );
}
