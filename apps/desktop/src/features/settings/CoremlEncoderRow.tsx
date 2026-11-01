/**
 * 一条 whisper 目录项的 **CoreML 加速** 行（ADR-115 / PRD-14-020）：设置 › 模型页与首启引导
 * 共用。状态全由 Rust 下发——`coreml_available / coreml_installed / coreml_size_bytes /
 * coreml_partial_bytes` 来自模型条目，字节进度来自 `model://download-progress`
 * （`<id>#coreml`），阶段来自 `model://coreml-phase`；这里只把它们排成一行。
 *
 * 「转换」= 首次 ANE 编译，发生在子进程探针里，可达数分钟：这一段显示已用秒数，
 * 让用户知道它在动，而不是一个不动的按钮。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { cssVariables } from "../../lib/css-variables";
import { formatBytes } from "../../lib/format-bytes";
import type { CoremlPhase, ModelDownloadProgress, ModelEntryDto } from "../../ipc/bindings";
import type { ModelsController } from "./use-settings";

/** 进度事件里编码器下载的 id 后缀（Rust `install_coreml_encoder` 的 `install_key`）。 */
const COREML_SUFFIX = "#coreml";

/** 进度事件里编码器下载的 id 形态。 */
export function coremlProgressId(modelId: string): string {
  return `${modelId}${COREML_SUFFIX}`;
}

/**
 * 进度 id → 它属于哪个模型、是不是 CoreML 那一条。
 *
 * 消费进度表的地方不止这一行（引导底部的进度胶囊也在遍历它），而拼 id 的规则只该有
 * 一处：胶囊此前按原样把 id 当模型 id 去查名字，查不到就把 `whisper-tiny-bundled`
 * 这种内部标识直接显示给用户看。
 */
export function parseProgressId(progressId: string): { modelId: string; coreml: boolean } {
  return progressId.endsWith(COREML_SUFFIX)
    ? { modelId: progressId.slice(0, -COREML_SUFFIX.length), coreml: true }
    : { modelId: progressId, coreml: false };
}

function useElapsedSeconds(since: number | null): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (since === null) {
      return;
    }
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [since]);
  return since === null ? 0 : Math.max(0, Math.round((now - since) / 1000));
}

export function CoremlEncoderRow({
  model,
  progress,
  phase,
  models,
  compact = false,
}: {
  model: ModelEntryDto;
  progress: ModelDownloadProgress | undefined;
  phase: CoremlPhase | undefined;
  models: ModelsController;
  /** 引导里的紧凑形态（一行，不带说明段）。 */
  compact?: boolean;
}) {
  const { t, i18n } = useTranslation("settings");
  const busy = phase?.phase === "downloading" || phase?.phase === "extracting" || phase?.phase === "compiling";
  const elapsed = useElapsedSeconds(phase?.phase === "compiling" ? phase.started_at_ms : null);
  const weightsInstalled = model.status === "installed" || model.status === "bundled";

  if (!model.coreml_available) {
    return null;
  }

  const downloading = progress !== undefined && !progress.done;
  const percent =
    downloading && (progress.total_bytes ?? 0) > 0
      ? Math.min(100, Math.round(((progress.downloaded_bytes ?? 0) / (progress.total_bytes ?? 1)) * 100))
      : null;

  let status: string;
  if (phase?.phase === "compiling") {
    status = t("models.coreml.compiling", { seconds: elapsed });
  } else if (phase?.phase === "extracting") {
    status = t("models.coreml.extracting");
  } else if (downloading) {
    status = t("models.coreml.downloading", {
      done: formatBytes(progress.downloaded_bytes ?? 0, i18n.language),
      total: formatBytes(progress.total_bytes ?? null, i18n.language),
    });
  } else if (phase?.phase === "failed") {
    status = t("models.coreml.failed");
  } else if (model.coreml_installed) {
    status = t("models.coreml.installed");
  } else if (!weightsInstalled) {
    status = t("models.coreml.needs_weights");
  } else {
    status = t("models.coreml.not_installed", { size: formatBytes(model.coreml_size_bytes, i18n.language) });
  }

  return (
    <div className="luna-coreml-row" data-compact={compact || undefined} data-busy={busy || undefined}>
      <div className="luna-coreml-row__text">
        <span className="luna-coreml-row__label">{t("models.coreml.label")}</span>
        <span className="luna-coreml-row__status" role="status">
          {status}
        </span>
        {!compact && !model.coreml_installed && !busy ? (
          <span className="luna-coreml-row__hint">{t("models.coreml.hint")}</span>
        ) : null}
        {downloading ? (
          <span className="luna-coreml-row__bar" aria-hidden="true">
            <span
              className="luna-coreml-row__bar-fill"
              data-indeterminate={percent === null}
              style={percent === null ? undefined : cssVariables({ "--progress-inline-size": `${percent}%` })}
            />
          </span>
        ) : null}
      </div>
      <div className="luna-coreml-row__actions">
        {downloading ? (
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            onClick={() => void models.pauseInstall(coremlProgressId(model.id))}
          >
            {t("models.pause")}
          </button>
        ) : model.coreml_installed ? (
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            disabled={busy}
            onClick={() => void models.removeCoremlEncoder(model.id)}
          >
            {t("models.coreml.disable")}
          </button>
        ) : (
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            disabled={busy || !weightsInstalled}
            onClick={() => void models.installCoremlEncoder(model.id)}
          >
            {model.coreml_partial_bytes !== null
              ? t("models.resume_from", { value: formatBytes(model.coreml_partial_bytes, i18n.language) })
              : phase?.phase === "failed"
                ? t("models.coreml.retry")
                : t("models.coreml.enable")}
          </button>
        )}
      </div>
    </div>
  );
}
