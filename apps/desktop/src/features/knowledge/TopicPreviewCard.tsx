/**
 * 知识网络里主题节点的预览卡（PRD-16-062 / ADR-136 ④）：单击主题节点先看一眼——范围、
 * 一句摘要、三条站得住的结论与出处——再决定打不打开。
 *
 * 判定全在 Rust（`topic_preview`）：摘要与目录、地图同一条规则；只摆「非分歧、出处都是当前」
 * 的结论，来源已变的不在这里冒充定论；旧格式页只有页名与摘要。**不调用模型**。
 *
 * 「就本主题提问」暂不给：对话还不能按主题范围检索（路线图「本主题提问受对话路由阻塞」），
 * 给一颗点了只能落到全库对话的按钮，等于说了一句兑现不了的话。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type TopicPreviewDto } from "../../ipc/bindings";
import { IconButton } from "../../components/ui/IconButton";
import { CloseIcon } from "../../components/ui/Icon";
import { vaultErrorText } from "../../lib/vault-error";

type Load = { kind: "loading" } | { kind: "ready"; preview: TopicPreviewDto } | { kind: "error"; message: string };

export function TopicPreviewCard({
  path,
  onOpen,
  onClose,
}: {
  path: string;
  /** 打开这一页（主题阅读视图）。 */
  onOpen: (path: string) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation("workspace");
  const [load, setLoad] = useState<Load>({ kind: "loading" });

  useEffect(() => {
    let disposed = false;
    setLoad({ kind: "loading" });
    void commands.topicPreview(path).then((result) => {
      if (disposed) return;
      setLoad(
        result.status === "ok"
          ? { kind: "ready", preview: result.data }
          : { kind: "error", message: vaultErrorText(result.error) },
      );
    });
    return () => {
      disposed = true;
    };
  }, [path]);

  const title = load.kind === "ready" ? load.preview.title : null;
  return (
    <aside className="luna-topic-preview" aria-label={t("topicPreview.label")}>
      <header className="luna-topic-preview__header">
        <h3 className="luna-topic-preview__title">{title ?? t("common:status.loading")}</h3>
        <IconButton
          density="sm"
          label={t("topicPreview.close")}
          icon={<CloseIcon size={12} />}
          onClick={onClose}
        />
      </header>
      {load.kind === "loading" ? (
        <p className="luna-topic-preview__hint" role="status">
          {t("common:status.loading")}
        </p>
      ) : load.kind === "error" ? (
        <p className="luna-field-error" role="alert">
          {load.message}
        </p>
      ) : (
        <PreviewBody preview={load.preview} />
      )}
      <div className="luna-topic-preview__actions">
        <button type="button" className="luna-btn luna-btn--primary luna-btn--sm" onClick={() => onOpen(path)}>
          {t("topicPreview.open")}
        </button>
      </div>
    </aside>
  );
}

function PreviewBody({ preview }: { preview: TopicPreviewDto }) {
  const { t } = useTranslation("workspace");
  const scope = preview.scope;
  return (
    <>
      {scope !== null && scope.includes.length > 0 && (
        <p className="luna-topic-preview__hint">
          {t("topicPreview.scope", {
            tags: scope.includes.map((tag) => `#${tag}`).join(t("topicDraft.sourceSeparator")),
          })}
        </p>
      )}
      {preview.summary && <p className="luna-topic-preview__summary">{preview.summary}</p>}
      {scope !== null && (
        <p className="luna-topic-preview__hint">
          {preview.conflict_count > 0
            ? t("topicPreview.countsWithConflicts", {
                count: preview.claim_count,
                conflicts: preview.conflict_count,
              })
            : t("topicPreview.counts", { count: preview.claim_count })}
        </p>
      )}
      {preview.claims.length > 0 ? (
        <ul className="luna-topic-evidence__items">
          {preview.claims.map((claim, index) => (
            <li key={index} className="luna-topic-evidence__item">
              <span className="luna-topic-evidence__claim">{claim.text}</span>
              <span className="luna-topic-evidence__sources">
                {t("topicDraft.sources", {
                  titles: claim.citations
                    .map((citation) => citation.source_title)
                    .join(t("topicDraft.sourceSeparator")),
                })}
              </span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="luna-topic-preview__hint">
          {t(scope === null ? "topicPreview.legacy" : "topicPreview.noSettled")}
        </p>
      )}
    </>
  );
}
