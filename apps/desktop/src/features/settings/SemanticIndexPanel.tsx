/**
 * 语义索引面板（「知识」分组专属，PRD-16-028/029 / ADR-027）。
 *
 * 它回答三个问题，且**都用真数字回答**：
 * ① 这个能力此刻可用吗（开关真值 × 模型是否在场，两件事分开说）；
 * ② 索引建到什么程度了（已索引 N 篇 / M 段、常驻内存多少）；
 * ③ 建完剩下的要多久（**预估秒数由 Rust 按 S16 实测的 0.25 秒/片给**，不说"很快"）。
 *
 * **全量建库是任务不是设置项**，故不做成一个开关——与「全库重扫」同一裁决（PRD-06-035b）：
 * 预检给数字 → 确认 → 单例后台逐篇 → 进度可见 → 可取消 → 终局通知。S16 实测万片约 40 分钟，
 * 做成一个转圈等的模态是不可接受的。
 *
 * **缺模型给引导而不是报错**：一句理由 + 去「本地模型」页一键下载（随包推理引擎运行，
 * 不需要另装任何东西，ADR-143），而不是一个点了必然失败的「开始」按钮。判定（`available`）
 * 由 Rust 下发，前端不自己探活（ADR-003）。
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, events, type EmbedPreflightDto, type EmbedProgress } from "../../ipc/bindings";
import { DownloadIcon, SparklesIcon } from "../../components/ui/Icon";
import { formatBytes } from "../../lib/format-bytes";
import { vaultErrorText } from "../../lib/vault-error";
import { goTo } from "../../lib/navigate";

/**
 * 非运行态下，动作区该占哪一格。
 *
 * **「隐藏重建细节」与「已经全部索引完了」必须分开判**（PRD-09-072 / TDD-05 §7.6）：
 * 渐进披露只允许**少说**，绝不允许**改说法**。二者曾并进同一个三元表达式，于是标准模式下
 * 三十几篇待索引也会打出「都已是最新」——真机第七轮实测到的谎报。
 */
export function semanticActionSlot(state: {
  advancedMode: boolean;
  pending: number;
}): "rebuild" | "all-fresh" | "none" {
  if (state.advancedMode && state.pending > 0) return "rebuild";
  if (state.pending === 0) return "all-fresh";
  return "none";
}

/** 把秒数说成人话（「约 12 分钟」比「731 秒」可决策）：返回 i18n key 与它的计数。 */
function etaKey(seconds: number): { key: string; count: number } {
  if (seconds < 60) {
    return { key: "semantic.etaSeconds", count: seconds };
  }
  if (seconds < 3600) {
    return { key: "semantic.etaMinutes", count: Math.round(seconds / 60) };
  }
  return { key: "semantic.etaHours", count: Math.round((seconds / 3600) * 10) / 10 };
}

export function SemanticIndexPanel({ advancedMode }: { advancedMode: boolean }) {
  const { t, i18n } = useTranslation("settings");
  const [state, setState] = useState<EmbedPreflightDto | null>(null);
  const [progress, setProgress] = useState<EmbedProgress | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    const [preflight, running] = await Promise.all([
      commands.embedPreflight(),
      commands.embedRebuildStatus(),
    ]);
    setState(preflight.status === "ok" ? preflight.data : null);
    setProgress(running.status === "ok" ? running.data : null);
  }, []);

  useEffect(() => {
    void refresh();
    // 进度是流、快照是状态：重挂载靠 `embedRebuildStatus` 恢复，运行期靠事件推进。
    const unlisten = events.embedProgress.listen((event) => {
      setProgress(event.payload.finished ? null : event.payload);
      if (event.payload.finished) {
        void refresh();
      }
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [refresh]);

  if (state === null) {
    return null;
  }

  const running = progress !== null || state.running;
  // 只有「没装」这一种不可用给下载引导；引擎不可用给的是另一句话——把"没跑"说成"没装"
  // 会引导用户去重装一个已经装好的模型（readiness code 由 Rust 下发）。
  const showInstallGuide = state.readiness === "not_installed" && state.model.length > 0;

  return (
    // 面板与设置行同处「知识」一组，故它有自己的标题——那是这一组里的一节，得有名字
    // （此前是一个无名的描边方块：读者读不出它在说哪件事）。
    <section
      id="settings-section-semantic_index"
      tabIndex={-1}
      className="luna-settings-panel luna-semantic-index"
      aria-label={t("semantic.label")}
    >
      <header className="luna-settings-panel__header">
        <h2>{t("semantic.label")}</h2>
      </header>
      {/* 不可用：按具体原因引导而不是报错。理由摆在界面上，命令可复制。 */}
      {!state.available ? (
        <div className="luna-semantic-index__guide">
          <p className="luna-semantic-index__title">
            <DownloadIcon size={14} />
            {showInstallGuide
              ? t("semantic.missingModel", { model: state.model })
              : t(`semantic.blocked.${state.readiness}`, { model: state.model })}
          </p>
          <p className="luna-settings-panel__hint">
            {showInstallGuide
              ? t("semantic.missingWhy")
              : t(`semantic.blocked.${state.readiness}_hint`, { model: state.model })}
          </p>
          {showInstallGuide ? (
            <div className="luna-settings-panel__actions">
              <button
                type="button"
                className="luna-btn luna-btn--primary"
                onClick={() => {
                  goTo("settings://local_models/models");
                }}
              >
                <DownloadIcon size={13} />
                {t("semantic.openModels")}
              </button>
            </div>
          ) : null}
          <div className="luna-settings-panel__actions">
            {/* 嵌入模型的选择器只在模型分配（PRD-09 §5 分层契约）；这里给跳转不复制选择器。 */}
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              onClick={() => {
                goTo("settings://ai_services");
              }}
            >
              {t("semantic.openAssignment")}
            </button>
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              onClick={() => void refresh()}
            >
              {t("semantic.recheck")}
            </button>
          </div>
        </div>
      ) : (
        <>
          {advancedMode ? (
            <dl className="luna-semantic-index__stats">
              <div>
                <dt>{t("semantic.indexed")}</dt>
                <dd>
                  {t("semantic.notesChunks", { notes: state.fresh, chunks: state.fresh_chunks })}
                </dd>
              </div>
              <div>
                <dt>{t("semantic.pending")}</dt>
                <dd>
                  {t("semantic.notesChunks", {
                    notes: state.pending,
                    chunks: state.pending_chunks,
                  })}
                </dd>
              </div>
              <div>
                <dt>{t("semantic.memory")}</dt>
                <dd>
                  {state.matrix_bytes > 0
                    ? formatBytes(state.matrix_bytes, i18n.language)
                    : t("semantic.notLoaded")}
                </dd>
              </div>
            </dl>
          ) : (
            <p className="luna-settings-panel__hint">
              {state.pending > 0
                ? t("semantic.standardPending", { count: state.pending })
                : t("semantic.standardReady")}
            </p>
          )}

          {/* 当前嵌入模型的只读摘要 + 跳转（PRD-09 §5 分层契约：标准模式保留摘要与跳转，
              选择器只在模型分配出现——这里绝不复制第二个选择器）。 */}
          <div className="luna-settings-panel__actions">
            <span className="luna-settings-panel__hint">
              {t("semantic.currentModel", { model: state.model })}
            </span>
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              onClick={() => {
                goTo("settings://ai_services");
              }}
            >
              {t("semantic.openAssignment")}
            </button>
          </div>

          {!state.enabled ? (
            <p className="luna-settings-panel__hint">{t("semantic.disabledHint")}</p>
          ) : null}

          {advancedMode && running && progress ? (
            <div className="luna-semantic-index__progress" role="status">
              <progress className="luna-progress" value={progress.done} max={progress.total} />
              <span>
                {t("semantic.progress", {
                  done: progress.done,
                  total: progress.total,
                  chunks: progress.chunks,
                })}
                {progress.failed > 0
                  ? t("semantic.progressFailed", { count: progress.failed })
                  : null}
              </span>
              <button
                type="button"
                className="luna-btn luna-btn--secondary"
                onClick={() => void commands.embedRebuildCancel()}
              >
                {t("semantic.cancel")}
              </button>
            </div>
          ) : semanticActionSlot({ advancedMode, pending: state.pending }) === "rebuild" ? (
            <div className="luna-settings-panel__actions">
              <button
                type="button"
                className="luna-btn luna-btn--primary"
                onClick={() => {
                  setError(null);
                  void commands.embedRebuildStart().then((result) => {
                    if (result.status !== "ok") {
                      setError(vaultErrorText(result.error));
                    } else {
                      void refresh();
                    }
                  });
                }}
              >
                <SparklesIcon size={14} />
                {t("semantic.build", { count: state.pending })}
              </button>
              {/* 预估如实给出：S16 实测万片约 40 分钟，用户要据此决定现在点不点。 */}
              <span className="luna-settings-panel__hint">
                {t("semantic.eta", {
                  eta: t(etaKey(state.eta_seconds).key, {
                    count: etaKey(state.eta_seconds).count,
                  }),
                })}
              </span>
            </div>
          ) : semanticActionSlot({ advancedMode, pending: state.pending }) === "all-fresh" ? (
            <p className="luna-settings-panel__hint">{t("semantic.allFresh")}</p>
          ) : null}

          {advancedMode && state.empty > 0 ? (
            <p className="luna-settings-panel__hint">
              {t("semantic.emptyNotes", { count: state.empty })}
            </p>
          ) : null}
          {error ? (
            <p className="luna-semantic-index__error" role="alert">
              {error}
            </p>
          ) : null}
        </>
      )}
    </section>
  );
}
