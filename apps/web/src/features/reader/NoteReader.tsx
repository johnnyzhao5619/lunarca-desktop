/**
 * 打开一个不是笔记的文件（PRD-21-013 / 014 / 015）：图片直接显示；录音与视频只给名称、大小与「请在桌面端播放」；
 * 其它附件给名称、大小与「下载」（在 Worker 里解密拼好、以原名交给浏览器下载，超过网页上限的请到桌面端打开）。
 * 笔记归编辑器（`features/editor/NoteEditor.tsx`，含阅读模式）。
 */

import { useEffect, useState } from "react";
import { saveErrorText } from "../../lib/error-message";
import { WorkerError } from "../../worker/worker-client";
import { useTranslation } from "react-i18next";
import { classifyAssetTarget } from "@lunarca/editor-core/asset-kind";
import { formatBytes } from "@lunarca/editor-core/format-bytes";
import type { FileSummary } from "../../worker/protocol";
import type { WorkerClient } from "../../worker/worker-client";

export interface NoteReaderProps {
  client: WorkerClient;
  file: FileSummary;
  /** 这个文件在别处改了（SSE 带来的变更）：每变一次加一，图片重新取。 */
  revision: number;
}

export function NoteReader({ client, file, revision }: NoteReaderProps) {
  const { t, i18n } = useTranslation("web");
  const assetKind = classifyAssetTarget(file.path);
  const meta = t("reader.fileMeta", { name: file.name, size: formatBytes(file.size, i18n.language) });

  if (assetKind === "audio" || assetKind === "video") {
    return (
      <section className="luna-web-reader__placeholder" data-testid="media-placeholder">
        <p className="luna-web-reader__file">{meta}</p>
        <p className="luna-web-hint">{t("reader.mediaHint")}</p>
      </section>
    );
  }
  if (assetKind === "image") {
    return (
      <section className="luna-web-reader__image">
        <ImageFile key={revision} client={client} file={file} />
        <DownloadButton client={client} file={file} />
      </section>
    );
  }
  return (
    <section className="luna-web-reader__placeholder" data-testid="attachment-placeholder">
      <p className="luna-web-reader__file">{meta}</p>
      <DownloadButton client={client} file={file} />
    </section>
  );
}

/** 下载完多久交还 Blob URL（浏览器开始下载之后就不再需要它）。 */
const DOWNLOAD_RELEASE_MS = 10_000;

/** 「下载」：Worker 解密拼好的 Blob URL 以原名交给浏览器，用完交还。 */
function DownloadButton({ client, file }: { client: WorkerClient; file: FileSummary }) {
  const { t, i18n } = useTranslation("web");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  async function download() {
    setBusy(true);
    setProblem(null);
    try {
      const done = await client.call("download", { file_id: file.file_id });
      const anchor = document.createElement("a");
      anchor.href = done.url;
      anchor.download = done.name;
      anchor.rel = "noopener";
      document.body.append(anchor);
      anchor.click();
      anchor.remove();
      window.setTimeout(() => void client.call("release_asset", { url: done.url }).catch(() => {}), DOWNLOAD_RELEASE_MS);
    } catch (error) {
      const { code, param } = error instanceof WorkerError ? { code: error.code, param: error.param } : { code: "internal", param: undefined };
      setProblem(saveErrorText(t, code, param, i18n.language));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <button type="button" className="luna-web-button" disabled={busy} onClick={() => void download()}>
        {busy ? t("reader.downloading") : t("reader.download")}
      </button>
      {problem ? (
        <p className="luna-web-error" role="alert">
          {problem}
        </p>
      ) : null}
    </>
  );
}

/** 直接打开的一张图（不是笔记里的图）：以它自己为锚、按文件名解析回它本身。 */
function ImageFile({ client, file }: { client: WorkerClient; file: FileSummary }) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    let created: string | null = null;
    void client.call("asset_url", { note_path: file.path, src: file.name }).then(
      (next) => {
        if (active) {
          created = next;
          setUrl(next);
        } else {
          void client.call("release_asset", { url: next }).catch(() => {});
        }
      },
      () => {},
    );
    return () => {
      active = false;
      if (created) void client.call("release_asset", { url: created }).catch(() => {});
    };
  }, [client, file.path, file.name]);
  return url ? <img className="luna-web-reader__picture" src={url} alt={file.name} /> : null;
}
