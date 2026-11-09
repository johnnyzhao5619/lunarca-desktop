/**
 * 未被引用的附件（PRD-06-012 ②，设置「笔记库」页；笔记库对标轮 V2）。
 *
 * 列出库里没有任何笔记引用的图片与文件，按体积倒序、给合计；用户勾选后按当前删除策略
 * 移走（可还原）。**只列不删**，从不自动清理。「被引用」的判定全在 Rust（与改名重构同一个
 * 解析器，回收站里的笔记与历史版本的引用也算），移走那一刻 Rust 再判一次——列出之后被
 * 引用上的会原样留下。前端零业务（ADR-003）。
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useConfirm } from "../../components/ConfirmHost";
import { commands, type OrphanReportDto } from "../../ipc/bindings";
import { formatBytes } from "@lunarca/editor-core/format-bytes";
import { vaultErrorText } from "../../lib/vault-error";
import { useDeleteConsequence } from "../../hooks/use-delete-mode";
import { HelpDetails } from "../../components/ui/HelpDetails";

type Phase =
  | { kind: "idle" }
  | { kind: "scanning" }
  | { kind: "listed"; report: OrphanReportDto }
  | { kind: "error"; message: string };

function splitPath(path: string): { name: string; dir: string } {
  const at = path.lastIndexOf("/");
  return at < 0 ? { name: path, dir: "" } : { name: path.slice(at + 1), dir: path.slice(0, at) };
}

export function OrphanAssetsPanel() {
  const { t, i18n } = useTranslation("settings");
  const ask = useConfirm();
  const consequence = useDeleteConsequence();
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  const [picked, setPicked] = useState<ReadonlySet<string>>(new Set());
  const [feedback, setFeedback] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  function scan() {
    setPhase({ kind: "scanning" });
    setPicked(new Set());
    void commands.orphanAssets().then((result) => {
      setPhase(
        result.status === "ok"
          ? { kind: "listed", report: result.data }
          : { kind: "error", message: vaultErrorText(result.error) },
      );
    });
  }

  function toggle(path: string, on: boolean) {
    setPicked((current) => {
      const next = new Set(current);
      if (on) next.add(path);
      else next.delete(path);
      return next;
    });
  }

  async function removePicked(report: OrphanReportDto) {
    const paths = report.items.filter((item) => picked.has(item.path)).map((item) => item.path);
    if (paths.length === 0) return;
    const bytes = report.items
      .filter((item) => picked.has(item.path))
      .reduce((sum, item) => sum + (item.bytes ?? 0), 0);
    const ok = await ask({
      title: t("orphan_assets.confirm_title", { n: paths.length }),
      description: t("orphan_assets.confirm_desc", {
        size: formatBytes(bytes, i18n.language),
        consequence,
      }),
      confirmLabel: t("orphan_assets.remove_confirm"),
      destructive: true,
    });
    if (!ok) return;
    setBusy(true);
    const result = await commands.removeOrphanAssets(paths);
    setBusy(false);
    if (result.status !== "ok") {
      setFeedback(vaultErrorText(result.error));
      return;
    }
    const parts = [t("orphan_assets.removed", { n: result.data.removed.length })];
    if (result.data.kept.length > 0) {
      parts.push(t("orphan_assets.kept", { n: result.data.kept.length }));
    }
    if (result.data.failed.length > 0) {
      parts.push(t("orphan_assets.failed", { n: result.data.failed.length }));
    }
    setFeedback(parts.join(" "));
    scan();
  }

  const report = phase.kind === "listed" ? phase.report : null;
  const allPicked = report !== null && report.items.length > 0 && picked.size === report.items.length;

  return (
    <section
      id="settings-section-orphan_assets"
      tabIndex={-1}
      className="luna-settings-panel"
      aria-label={t("orphan_assets.label")}
    >
      <header className="luna-settings-panel__header">
        <h2>{t("orphan_assets.label")}</h2>
      </header>
      <p className="luna-settings-panel__hint">{t("orphan_assets.hint")}</p>
      {/* 作用范围与保护规则按需展开（2026-10-08 W12）：一句常显说清这是什么，细节不压在首屏。 */}
      <HelpDetails summary={t("orphan_assets.scope_summary")}>
        <p>{t("orphan_assets.scope")}</p>
      </HelpDetails>
      <div className="luna-settings-panel__actions">
        <span className="luna-settings-panel__hint" role="status">
          {phase.kind === "scanning"
            ? t("orphan_assets.scanning")
            : phase.kind === "error"
              ? phase.message
              : report
                ? report.items.length === 0
                  ? t("orphan_assets.none")
                  : t("orphan_assets.summary", {
                      n: report.items.length,
                      size: formatBytes(report.total_bytes, i18n.language),
                    })
                : null}
        </span>
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          disabled={phase.kind === "scanning" || busy}
          onClick={scan}
        >
          {report ? t("orphan_assets.rescan") : t("orphan_assets.scan")}
        </button>
      </div>
      {report && report.items.length > 0 && (
        <>
          <label className="luna-settings-panel__toggle">
            <input
              type="checkbox"
              checked={allPicked}
              onChange={(event) =>
                setPicked(event.target.checked ? new Set(report.items.map((item) => item.path)) : new Set())
              }
            />
            <span>{t("orphan_assets.select_all")}</span>
          </label>
          <ul className="luna-settings-panel__rows">
            {report.items.map((item) => {
              const { name, dir } = splitPath(item.path);
              return (
                <li key={item.path} className="luna-settings-panel__row">
                  <label className="luna-settings-panel__toggle" title={item.path}>
                    <input
                      type="checkbox"
                      checked={picked.has(item.path)}
                      onChange={(event) => toggle(item.path, event.target.checked)}
                    />
                    <span className="luna-settings-panel__row-label">
                      <span className="luna-settings-panel__row-title">{name}</span>
                      {dir && <span className="luna-settings-panel__row-desc">{dir}</span>}
                    </span>
                  </label>
                  <span className="luna-settings-panel__row-value">
                    {formatBytes(item.bytes, i18n.language)}
                  </span>
                </li>
              );
            })}
          </ul>
          <div className="luna-settings-panel__actions">
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              disabled={picked.size === 0 || busy}
              onClick={() => void removePicked(report)}
            >
              {t("orphan_assets.remove", { n: picked.size })}
            </button>
          </div>
        </>
      )}
      {feedback && (
        <p className="luna-settings-panel__hint" role="status">
          {feedback}
        </p>
      )}
    </section>
  );
}
