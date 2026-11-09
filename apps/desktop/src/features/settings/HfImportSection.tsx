/**
 * 按 Hugging Face 仓库 id 添加 GGUF 模型（ADR-144 ③，高级模式）。
 *
 * 只认 `owner/name`，不接受任意 URL；清单钉在仓库当前提交上，下载时服务端重新核对文件并边下边
 * 校验 sha256，下完走与本地导入同一道闸。前端零业务（ADR-003）：只发命令、渲染清单与进度。
 * 提交只走按钮、不绑 Enter——避开输入法上屏那一下被当成提交。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { commands, events, type HfRepoDto } from "../../ipc/bindings";
import { formatBytes } from "@lunarca/editor-core/format-bytes";
import { FactRow } from "../../components/ui/FactRow";
import { noticeText } from "./model-notice";

export function HfImportSection({ onImported }: { onImported?: () => void }) {
  const { t, i18n } = useTranslation("settings");
  const [repo, setRepo] = useState("");
  const [listing, setListing] = useState<HfRepoDto | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  /** 正在下载的文件：`hf:<repo>/<path>` → 百分比。 */
  const [progress, setProgress] = useState<{ key: string; percent: number } | null>(null);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    const unlisten = events.modelDownloadProgress.listen((event) => {
      const p = event.payload;
      if (!p.model_id.startsWith("hf:") || !mounted.current) return;
      const total = p.total_bytes ?? 0;
      const received = p.downloaded_bytes ?? 0;
      setProgress(p.done ? null : { key: p.model_id, percent: total > 0 ? Math.round((received / total) * 100) : 0 });
    });
    return () => {
      mounted.current = false;
      void unlisten.then((stop) => stop());
    };
  }, []);

  const list = useCallback(async () => {
    setNotice(null);
    setListing(null);
    setBusy(true);
    const result = await commands.hfRepoGgufs(repo.trim());
    if (!mounted.current) return;
    setBusy(false);
    if (result.status === "ok") {
      setListing(result.data);
    } else {
      setNotice(noticeText(result.error, t));
    }
  }, [repo, t]);

  const importFile = useCallback(
    async (path: string) => {
      if (!listing) return;
      setNotice(null);
      setBusy(true);
      const result = await commands.importHfGguf(listing.repo, path);
      if (!mounted.current) return;
      setBusy(false);
      setProgress(null);
      if (result.status === "error") {
        setNotice(noticeText(result.error, t));
        return;
      }
      setNotice(
        t(result.data.matched_catalog ? "models.notice.MODEL_IMPORTED_AS_CATALOG" : "models.notice.MODEL_IMPORTED", {
          detail: result.data.name,
          role: t(`models.import_role.${result.data.role}`),
        }),
      );
      onImported?.();
      void list();
    },
    [listing, list, onImported, t],
  );

  return (
    <section className="luna-settings-panel__section" aria-label={t("models.hf_import.title")}>
      <h2 className="luna-settings-panel__section-title">{t("models.hf_import.title")}</h2>
      <p className="luna-settings-panel__hint">{t("models.hf_import.hint")}</p>
      <div className="luna-settings-panel__actions">
        <input
          type="text"
          className="luna-input"
          value={repo}
          placeholder={t("models.hf_import.placeholder")}
          aria-label={t("models.hf_import.placeholder")}
          spellCheck={false}
          onChange={(e) => setRepo(e.currentTarget.value)}
        />
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          disabled={busy || repo.trim() === ""}
          onClick={() => void list()}
        >
          {t("models.hf_import.list")}
        </button>
      </div>
      {listing ? (
        <div className="luna-settings-panel__rows">
          <FactRow
            label={t("models.hf_import.revision_label")}
            value={t("models.hf_import.revision", {
              revision: listing.revision.slice(0, 12),
              license: listing.license ?? t("models.hf_import.license_unknown"),
            })}
          />
          {listing.files.length === 0 ? (
            <p className="luna-settings-panel__hint">{t("models.hf_import.empty")}</p>
          ) : (
            listing.files.map((file) => {
              const key = `hf:${listing.repo}/${file.path}`;
              const size = formatBytes(file.size_bytes, i18n.language);
              return (
                <div key={file.path} className="luna-settings-panel__actions">
                  <FactRow
                    label={file.path}
                    value={
                      file.catalog_name
                        ? t("models.local_engine.row_catalog_match", { size, name: file.catalog_name })
                        : size
                    }
                  />
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary"
                    disabled={busy || file.already_installed}
                    onClick={() => void importFile(file.path)}
                  >
                    {file.already_installed
                      ? t("models.local_engine.import_installed")
                      : progress?.key === key
                        ? t("models.hf_import.downloading", { percent: progress.percent })
                        : t("models.hf_import.download")}
                  </button>
                </div>
              );
            })
          )}
        </div>
      ) : null}
      {notice ? (
        <p className="luna-settings__notice" role="status">
          {notice}
        </p>
      ) : null}
    </section>
  );
}
