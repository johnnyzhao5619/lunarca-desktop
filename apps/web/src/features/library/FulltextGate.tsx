/**
 * 全文搜索与标签共用的那一道门（PRD-21-041）：写明「搜索在你的浏览器里完成」，用户点了才下载全部正文、建索引；
 * 下载期间给进度、可取消。
 */

import { useTranslation } from "react-i18next";
import { saveErrorText } from "../../lib/error-message";
import type { Fulltext } from "./use-fulltext";

export function FulltextGate({ fulltext }: { fulltext: Fulltext }) {
  const { t, i18n } = useTranslation("web");
  const { state, problem } = fulltext;
  const building = state.state === "building";
  return (
    <section className="luna-web-fulltext" data-testid="fulltext-gate">
      <p className="luna-web-fulltext__privacy">{t("fulltext.privacy")}</p>
      {building ? (
        <div className="luna-web-fulltext__progress" role="status" aria-live="polite">
          <progress className="luna-web-fulltext__bar" max={Math.max(state.total, 1)} value={state.done} />
          <span>{t("fulltext.progress", { done: state.done, total: state.total })}</span>
          <button type="button" className="luna-web-button luna-web-button--quiet" onClick={fulltext.cancel}>
            {t("fulltext.cancel")}
          </button>
        </div>
      ) : (
        <button type="button" className="luna-web-button luna-web-button--primary" onClick={fulltext.start}>
          {t("fulltext.start")}
        </button>
      )}
      {problem ? (
        <p className="luna-web-error" role="alert">
          {t("fulltext.failed", { reason: saveErrorText(t, problem.code, problem.param, i18n.language) })}
        </p>
      ) : null}
    </section>
  );
}
