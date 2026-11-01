// 非 Markdown 文件预览视图（PRD-06-007，Obsidian 附件预览标签对标）：
// 图片/音频/视频**内部预览**（asset 协议 / 受管 HTTP-Range 服务），PDF/文档等
// 二进制**用外部程序打开 + 只读信息**。业务在 Rust，前端只渲染与发命令（ADR-003）。

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "../../ipc/bindings";
import { vaultMediaStreamUrl, vaultFileAssetSrc } from "./media-src";
import { ExternalWindowIcon, FileTextIcon, ImageIcon } from "../../components/ui/Icon";
import type { AssetKind } from "./asset-kind";
import { fileNameOf } from "../../lib/note-path";

interface AssetViewProps {
  path: string;
  kind: AssetKind;
}

export function AssetView({ path, kind }: AssetViewProps) {
  const { t } = useTranslation("editor");
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let active = true;
    setSrc(null);
    setFailed(false);
    const loader =
      kind === "image"
        ? vaultFileAssetSrc(path)
        : kind === "audio" || kind === "video"
          ? vaultMediaStreamUrl(path)
          : null;
    if (!loader) {
      return;
    }
    loader
      .then((url) => {
        if (active) setSrc(url);
      })
      .catch(() => {
        if (active) setFailed(true);
      });
    return () => {
      active = false;
    };
  }, [path, kind]);

  const openExternal = () => {
    void commands.openVaultPath(path);
  };

  return (
    <section className="luna-asset-view" data-kind={kind}>
      <header className="luna-asset-view__header">
        <span className="luna-asset-view__name" title={path}>
          {fileNameOf(path)}
        </span>
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-asset-view__open"
          onClick={openExternal}
        >
          <ExternalWindowIcon size={13} />
          {t("asset.openExternal")}
        </button>
      </header>

      <div className="luna-asset-view__body">
        {failed ? (
          <div className="luna-asset-view__fallback">
            <FileTextIcon size={40} className="luna-asset-view__fallback-icon" />
            <p>{t("asset.loadFailed")}</p>
            <button type="button" className="luna-btn luna-btn--secondary" onClick={openExternal}>
              {t("asset.openExternal")}
            </button>
          </div>
        ) : kind === "image" ? (
          src ? (
            <img className="luna-asset-view__image" src={src} alt={fileNameOf(path)} />
          ) : (
            <div className="luna-asset-view__loading" aria-hidden="true" />
          )
        ) : kind === "audio" ? (
          src ? (
            <audio className="luna-asset-view__audio" src={src} controls crossOrigin="anonymous" />
          ) : (
            <div className="luna-asset-view__loading" aria-hidden="true" />
          )
        ) : kind === "video" ? (
          src ? (
            <video className="luna-asset-view__video" src={src} controls crossOrigin="anonymous" />
          ) : (
            <div className="luna-asset-view__loading" aria-hidden="true" />
          )
        ) : (
          // pdf / 二进制文档：不内嵌，走外部程序打开（离线纪律：不发起网络加载）。
          <div className="luna-asset-view__fallback">
            {kind === "pdf" ? (
              <FileTextIcon size={40} className="luna-asset-view__fallback-icon" />
            ) : (
              <ImageIcon size={40} className="luna-asset-view__fallback-icon" />
            )}
            <p>{t(kind === "pdf" ? "asset.pdfHint" : "asset.binaryHint")}</p>
            <button type="button" className="luna-btn luna-btn--primary" onClick={openExternal}>
              <ExternalWindowIcon size={14} />
              {t("asset.openExternal")}
            </button>
          </div>
        )}
      </div>
    </section>
  );
}
