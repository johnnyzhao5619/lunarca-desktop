/**
 * 「相关」面板（PRD-16-013/014）：未链接提及 + 相关笔记。
 *
 * 信任阶梯的**第一级**：全部是只读发现，零 LLM、零嵌入、零自动写入。用户点了「链接」
 * 才写，且写入前 Rust 会逐字节复核那段文字还在（面板数据可能已陈旧）。
 *
 * 资料笔记（录音/转写/对话）照常显示发现，但**不给写入动作**——往逐字稿里插链接会污染
 * 原文。判定 `can_link` 由 Rust 下发，前端不自行判断"哪几种算资料"（ADR-003）。
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, events, type RelatedNotesDto } from "../../ipc/bindings";
import { LinkIcon, SparklesIcon, WikiLinkIcon } from "@lunarca/editor-core/Icon";
import { EmptyState } from "../../components/ui/EmptyState";
import { notePathLabel } from "../../lib/note-path";

interface RelatedPanelProps {
  path: string | null;
  onOpenPath?: (path: string) => void;
}

export function RelatedPanel({ path, onOpenPath }: RelatedPanelProps) {
  const { t } = useTranslation("workspace");
  const [data, setData] = useState<RelatedNotesDto | null>(null);
  const [loading, setLoading] = useState(false);
  const [linking, setLinking] = useState<string | null>(null);

  const reload = useCallback(async () => {
    if (!path) {
      setData(null);
      return;
    }
    setLoading(true);
    const result = await commands.relatedNotes(path);
    setData(result.status === "ok" ? result.data : null);
    setLoading(false);
  }, [path]);

  useEffect(() => {
    void reload();
  }, [reload]);

  // 正文变化后重算：链接建立/笔记保存都会改变「还有哪些没链接」。
  useEffect(() => {
    const unlisten = events.noteWritten.listen((event) => {
      if (event.payload.path === path) {
        void reload();
      }
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [path, reload]);

  const link = useCallback(
    async (mentionIndex: number) => {
      if (!path || !data) {
        return;
      }
      const mention = data.mentions[mentionIndex];
      setLinking(`${mention.line}:${mention.start}`);
      await commands.linkMention(path, mention);
      setLinking(null);
      await reload();
    },
    [data, path, reload],
  );

  if (!path) {
    return <EmptyState icon={<SparklesIcon size={24} />} title={t("panel.noSelection")} />;
  }
  if (loading && !data) {
    return <p className="luna-empty">{t("common:status.loading")}</p>;
  }
  if (!data) {
    return <EmptyState icon={<SparklesIcon size={24} />} title={t("panel.empty")} />;
  }

  const nothing = data.mentions.length === 0 && data.related.length === 0;
  if (nothing) {
    return <EmptyState icon={<SparklesIcon size={24} />} title={t("related.empty")} />;
  }

  return (
    <div className="luna-related">
      {data.mentions.length > 0 ? (
        <section className="luna-related__section">
          <h4 className="luna-related__heading">
            <WikiLinkIcon size={12} />
            {t("related.mentions")}
          </h4>
          <ul className="luna-related__mentions">
            {data.mentions.map((mention, index) => (
              <li key={`${mention.line}:${mention.start}`}>
                <span className="luna-related__mention-line" title={mention.line_text}>
                  {mention.line_text.trim()}
                </span>
                <div className="luna-related__mention-actions">
                  <span className="luna-related__mention-target">
                    {notePathLabel(mention.target_path)}
                  </span>
                  {data.can_link ? (
                    <button
                      type="button"
                      className="luna-btn luna-btn--secondary"
                      disabled={linking !== null}
                      onClick={() => void link(index)}
                    >
                      <LinkIcon size={12} />
                      {t("related.link")}
                    </button>
                  ) : null}
                </div>
              </li>
            ))}
          </ul>
          {data.can_link ? null : <p className="luna-related__readonly">{t("related.readonly")}</p>}
        </section>
      ) : null}

      {data.related.length > 0 ? (
        <section className="luna-related__section">
          <h4 className="luna-related__heading">{t("related.notes")}</h4>
          <ul className="luna-panel-link-list">
            {data.related.map((ref) => (
              <li key={ref.path}>
                <button
                  type="button"
                  className="luna-panel-link-list__item"
                  title={ref.path}
                  onClick={() => onOpenPath?.(ref.path)}
                >
                  <span>{notePathLabel(ref.path)}</span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
}
