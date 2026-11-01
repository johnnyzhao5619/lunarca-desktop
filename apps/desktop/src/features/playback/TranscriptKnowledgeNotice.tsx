/**
 * 录音/转写页上关于「它能不能成为主题依据」的一句话（PRD-16-058 / PRD-16-065，ADR-136 ②）。
 *
 * 两种情形由 Rust 判定（`transcript_knowledge_notice`），前端只渲染：
 * - **只有实时稿**：录音时的预览稿不作为主题依据，重新转写一次即可——此前这件事没有任何
 *   地方说出来，没装批量转写模型的用户永远不知道自己的录音进不了主题。
 * - **在这里问一次**：已完成权威转写、而这台设备在当前 AI 落点下还没回答过。授权原本只能
 *   在某个主题页的面板里找到；现在在它发生的地方问——用途、在哪处理、在哪撤回一次说清，
 *   「允许」与「暂不」同等分量，暂不之后不再逐条录音地问。
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  type TranscriptKnowledgeNoticeDto,
  type TranscriptSourceDecisionDto,
} from "../../ipc/bindings";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { vaultErrorText } from "../../lib/vault-error";

export function TranscriptKnowledgeNotice({
  noteRel,
  onRetranscribe,
  retranscribing,
}: {
  noteRel: string;
  /** 重新转写（没有本地媒体时为 `null`：只说明，不给点了没用的按钮）。 */
  onRetranscribe: (() => void) | null;
  retranscribing: boolean;
}) {
  const { t } = useTranslation("playback");
  const [notice, setNotice] = useState<TranscriptKnowledgeNoticeDto>({ status: "none" });
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    void commands.transcriptKnowledgeNotice(noteRel).then((result) => {
      setNotice(result.status === "ok" ? result.data : { status: "none" });
    });
  }, [noteRel]);

  useEffect(() => {
    setError(null);
    load();
  }, [load]);
  // 重新转写完成、或在别处改了授权之后跟着变。
  useVaultReload(load);

  const decide = (decision: TranscriptSourceDecisionDto) => {
    void commands.setTranscriptSources(decision).then((result) => {
      if (result.status === "error") {
        setError(vaultErrorText(result.error));
        return;
      }
      setError(null);
      load();
    });
  };

  if (notice.status === "none") {
    return null;
  }
  if (notice.status === "preview_only") {
    return (
      <div className="luna-playback__knowledge-notice" role="note">
        <p>{t("knowledge.previewOnly")}</p>
        {onRetranscribe !== null && (
          <div className="luna-playback__no-media-actions">
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              disabled={retranscribing}
              onClick={onRetranscribe}
            >
              {retranscribing ? t("secondPass.running") : t("secondPass.run")}
            </button>
          </div>
        )}
      </div>
    );
  }
  return (
    <div
      className="luna-playback__knowledge-notice"
      role="group"
      aria-label={t("knowledge.askTitle")}
    >
      <p>
        <strong>{t("knowledge.askTitle")}</strong>
      </p>
      <p>{t("knowledge.askBody", { where: t(`knowledge.where.${notice.location}`) })}</p>
      <div className="luna-playback__no-media-actions">
        {/* 两个选择同等分量：不预选、不把「允许」染成主按钮（同意不该被设计成默认）。 */}
        <button type="button" className="luna-btn luna-btn--secondary" onClick={() => decide("allow")}>
          {t("knowledge.allow")}
        </button>
        <button type="button" className="luna-btn luna-btn--secondary" onClick={() => decide("not_now")}>
          {t("knowledge.notNow")}
        </button>
      </div>
      {error !== null && (
        <p className="luna-playback__no-media-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
