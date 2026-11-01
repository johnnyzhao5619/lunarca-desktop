/**
 * 打印窗口（PDF 导出，PRD-11-011 / TDD-10 §1.2）：`note-print-<n>` 窗口挂载时按自身 label
 * 取笔记路径，用**同一份**阅读视图渲染，图片加载完即唤起系统打印面板——macOS 在那里
 * 「存储为 PDF」。不引排版引擎、不另写一套渲染。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { commands, type EmbedHitDto } from "../../ipc/bindings";
import { noteStem } from "../../lib/note-path";
import { ReadingView, resolveEmbedsFor } from "./ReadingView";
import { prepareRichContent } from "../../lib/rich-content";

/** 等图片与字体就位再打印：打印面板拿到的是当前渲染快照。 */
async function waitForAssets(): Promise<void> {
  await document.fonts.ready;
  const images = Array.from(document.images);
  await Promise.all(
    images.map(
      (img) =>
        new Promise<void>((resolve) => {
          if (img.complete) {
            resolve();
            return;
          }
          img.addEventListener("load", () => resolve(), { once: true });
          img.addEventListener("error", () => resolve(), { once: true });
        }),
    ),
  );
}

export function NotePrintView() {
  const { t } = useTranslation("editor");
  const [notePath, setNotePath] = useState<string | null>(null);
  const [content, setContent] = useState<string | null>(null);
  const [embeds, setEmbeds] = useState<ReadonlyMap<string, EmbedHitDto>>(() => new Map());
  const [printed, setPrinted] = useState(false);

  useEffect(() => {
    void commands.noteWindowPath().then((result) => {
      if (result.status === "ok" && result.data) {
        setNotePath(result.data);
      }
    });
  }, []);

  useEffect(() => {
    if (!notePath) {
      return;
    }
    void getCurrentWindow().setTitle(noteStem(notePath));
    void commands.readNote(notePath).then(async (result) => {
      if (result.status === "ok") {
        // 公式与图表先排好再渲染：打印面板拿到的是那一刻的快照。
        await prepareRichContent(result.data.content);
        // 嵌入展开进打印稿（与 HTML 导出同一口径）。
        setEmbeds(await resolveEmbedsFor(notePath, result.data.content));
        setContent(result.data.content);
      }
    });
  }, [notePath]);

  // 渲染出来之后再打印一次；之后留一颗按钮给用户重打（比如换了打印设置）。
  useEffect(() => {
    if (content === null || printed) {
      return;
    }
    let cancelled = false;
    void waitForAssets().then(() => {
      if (cancelled) {
        return;
      }
      setPrinted(true);
      void commands.printCurrentWindow();
    });
    return () => {
      cancelled = true;
    };
  }, [content, printed]);

  return (
    <div className="luna-print">
      <header className="luna-print__bar">
        <span className="luna-print__hint">{t("print.hint")}</span>
        <div className="luna-print__actions">
          <button
            type="button"
            className="luna-btn luna-btn--primary"
            disabled={content === null}
            onClick={() => void commands.printCurrentWindow()}
          >
            {t("print.again")}
          </button>
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            onClick={() => void getCurrentWindow().close()}
          >
            {t("print.close")}
          </button>
        </div>
      </header>
      <main className="luna-export__page">
        {content !== null && notePath ? (
          <ReadingView content={content} notePath={notePath} staticExport embeds={embeds} />
        ) : (
          <p className="luna-print__loading">{t("print.loading")}</p>
        )}
      </main>
    </div>
  );
}
