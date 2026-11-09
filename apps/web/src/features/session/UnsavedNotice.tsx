/**
 * 会话结束（吊销、挤掉）或被锁定时，本页还有没存上的改动（PRD-21-004「未保存的内容先提示」）：这时已经存不
 * 上了，能做的是告诉用户是哪几篇、让他把内容复制走，再去登录或解锁。正文不显示在屏幕上，只经剪贴板带走。
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { UnsavedDraft } from "../editor/session-registry";

export interface UnsavedNoticeProps {
  drafts: UnsavedDraft[];
  /** 是被锁定（之后去解锁），不是被退出（之后去登录）。 */
  locked: boolean;
  onContinue(): void;
}

export function UnsavedNotice({ drafts, locked, onContinue }: UnsavedNoticeProps) {
  const { t } = useTranslation("web");
  const [copied, setCopied] = useState<string | null>(null);

  async function copy(draft: UnsavedDraft) {
    try {
      await navigator.clipboard.writeText(draft.text);
      setCopied(draft.file_id);
    } catch {
      setCopied(null);
    }
  }

  return (
    <main className="luna-web-auth" aria-labelledby="web-unsaved-title" data-testid="unsaved-notice">
      <section className="luna-web-auth__card">
        <h1 id="web-unsaved-title" className="luna-web-auth__title">
          {locked ? t("unsaved.lockedTitle") : t("unsaved.title")}
        </h1>
        <p className="luna-web-auth__subtitle">{t("unsaved.body")}</p>
        <ul className="luna-web-unsaved">
          {drafts.map((draft) => (
            <li key={draft.file_id} className="luna-web-unsaved__item">
              <span className="luna-web-unsaved__path">{draft.path}</span>
              <button type="button" className="luna-web-button" onClick={() => void copy(draft)}>
                {copied === draft.file_id ? t("unsaved.copied") : t("unsaved.copy")}
              </button>
            </li>
          ))}
        </ul>
        <button type="button" className="luna-web-button luna-web-button--primary" onClick={onContinue}>
          {locked ? t("unsaved.continueLocked") : t("unsaved.continue")}
        </button>
      </section>
    </main>
  );
}
