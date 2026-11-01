/**
 * 笔记索引面板（「数据 / 隐私」分组，PRD-07-023 / TDD-06 §3）。
 *
 * 索引（`.luna/index.sqlite`）是从笔记文件派生的缓存：列表、搜索、标签、链接都从它读。
 * 它坏了或落后了，用户看到的是"这一页没能加载出来"或"明明有笔记却搜不到"——这里给
 * 一键「重建索引」：走与开库同一条后台扫描，进度经 `scan://progress` 推到状态栏与本面板，
 * 终态自动刷新工作区。**只读**：重建绝不改写任何笔记（Rust 侧保证，ADR-003 前端不判）。
 *
 * 扫描（开库那一轮或这里点的重建）随时可取消（PRD-06-004 ②）：已写进去的批次有效，下次
 * 打开库接着比对，不从头来。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, events } from "../../ipc/bindings";
import { vaultErrorText } from "../../lib/vault-error";

type IndexPhase =
  | { kind: "idle" }
  | { kind: "running"; indexed: number; total: number }
  | { kind: "done"; indexed: number }
  | { kind: "cancelled" }
  | { kind: "error"; message: string };

export function VaultIndexPanel() {
  const { t } = useTranslation("settings");
  const [phase, setPhase] = useState<IndexPhase>({ kind: "idle" });

  useEffect(() => {
    // 开库那一轮可能在面板挂载前就开始了：先取一次现状，免得进行中的扫描显示成空闲。
    let heard = false;
    void commands.vaultIndexStatus().then((status) => {
      if (!heard && status.scanning) {
        setPhase({ kind: "running", indexed: status.indexed, total: status.total });
      }
    });
    const unlisten = events.scanProgress.listen((event) => {
      heard = true;
      const progress = event.payload;
      setPhase(
        progress.cancelled
          ? { kind: "cancelled" }
          : progress.done
            ? { kind: "done", indexed: progress.indexed }
            : { kind: "running", indexed: progress.indexed, total: progress.total },
      );
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  function rebuild() {
    setPhase({ kind: "running", indexed: 0, total: 0 });
    void commands.rebuildVaultIndex().then((result) => {
      if (result.status === "error") {
        setPhase({ kind: "error", message: vaultErrorText(result.error) });
      }
    });
  }

  return (
    <section
      id="settings-section-vault_index"
      tabIndex={-1}
      className="luna-settings-panel"
      aria-label={t("vault_index.label")}
    >
      <header className="luna-settings-panel__header">
        <h2>{t("vault_index.label")}</h2>
      </header>
      <p className="luna-settings-panel__hint">{t("vault_index.hint")}</p>
      <div className="luna-settings-panel__actions">
        <span className="luna-settings-panel__hint" role="status">
          {phase.kind === "running"
            ? t("vault_index.running", { indexed: phase.indexed, total: phase.total })
            : phase.kind === "done"
              ? t("vault_index.done", { count: phase.indexed })
              : phase.kind === "cancelled"
                ? t("vault_index.cancelled")
                : phase.kind === "error"
                  ? phase.message
                  : null}
        </span>
        {phase.kind === "running" ? (
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            onClick={() => void commands.cancelVaultScan()}
          >
            {t("vault_index.cancel")}
          </button>
        ) : (
          <button type="button" className="luna-btn luna-btn--secondary" onClick={rebuild}>
            {t("vault_index.rebuild")}
          </button>
        )}
      </div>
    </section>
  );
}
