/**
 * 速记产物条（PRD-02-033）：一篇语音速记顶上并排列出它此刻拥有的产物——原始转录 /
 * 优化结果 / 二次转写——各带字数，用户可标一版为「采用」；留了原始录音时给一条回放。
 *
 * 「采用」只是标记（frontmatter `luna.adopted`），正文三节一字不动、导出与列表照旧；
 * 三种产物文本由 Rust 从正文读回，前端不解析正文（ADR-003）。不是速记时不渲染。
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type QuickNoteProductsDto } from "../../ipc/bindings";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { vaultMediaStreamUrl } from "../media/media-src";

const VERSIONS = ["raw", "refined", "second_pass"] as const;
type Version = (typeof VERSIONS)[number];

export function QuickNoteProductBar({ notePath }: { notePath: string }) {
  const { t } = useTranslation("editor");
  const [products, setProducts] = useState<QuickNoteProductsDto | null>(null);
  const [audioSrc, setAudioSrc] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    const result = await commands.quickNoteProducts(notePath);
    setProducts(result.status === "ok" ? result.data : null);
  }, [notePath]);

  useEffect(() => {
    void refresh();
  }, [refresh]);
  useVaultReload(() => void refresh());

  const mediaRel = products?.media_rel ?? null;
  useEffect(() => {
    let active = true;
    setAudioSrc(null);
    if (!mediaRel) {
      return;
    }
    void vaultMediaStreamUrl(mediaRel).then((url) => {
      if (active) {
        setAudioSrc(url);
      }
    });
    return () => {
      active = false;
    };
  }, [mediaRel]);

  if (!products) {
    return null;
  }
  const available = VERSIONS.filter((v) => products[v] !== null);
  if (available.length === 0 && !mediaRel) {
    return null;
  }

  function adopt(version: Version) {
    const next = products?.adopted === version ? null : version;
    void commands.setQuickNoteAdopted(notePath, next).then((result) => {
      if (result.status === "ok") {
        void refresh();
      }
    });
  }

  return (
    <div className="luna-quick-products" role="group" aria-label={t("quickNote.products")}>
      <div className="luna-quick-products__versions" role="radiogroup" aria-label={t("quickNote.adopted")}>
        {available.map((version) => {
          const item = products[version];
          const adopted = products.adopted === version;
          return (
            <button
              key={version}
              type="button"
              role="radio"
              aria-checked={adopted}
              className="luna-quick-products__chip"
              data-adopted={adopted || undefined}
              title={item ? item.text.slice(0, 200) : undefined}
              onClick={() => adopt(version)}
            >
              <span className="luna-quick-products__name">{t(`quickNote.version.${version}`)}</span>
              {item ? (
                <span className="luna-quick-products__chars">
                  {t("quickNote.chars", { count: item.chars })}
                </span>
              ) : null}
              {adopted ? (
                <span className="luna-quick-products__mark">{t("quickNote.adoptedMark")}</span>
              ) : null}
            </button>
          );
        })}
      </div>
      {/* 媒体服务校验 Origin：不带 crossOrigin 的媒体元素根本不发 Origin 头，会被 403 → 播放器「Error」。 */}
      {audioSrc ? (
        <audio
          className="luna-quick-products__audio"
          controls
          crossOrigin="anonymous"
          preload="none"
          src={audioSrc}
          aria-label={t("quickNote.audio")}
        />
      ) : null}
    </div>
  );
}
