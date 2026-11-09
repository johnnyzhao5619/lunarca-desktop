import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { useConfirm } from "../../components/ConfirmHost";
import { commands, type CloudError, type WebSessionDto } from "../../ipc/cloud-bindings";
import { errorText } from "../../lib/cloud-error";
import { formatInstant } from "../../lib/format-datetime";

/** 已登录的浏览器：逐个或全部退出（PRD-21-005）。 */
export function WebSessions() {
  const { t, i18n } = useTranslation("cloud");
  const ask = useConfirm();
  const mounted = useRef(false);
  const [sessions, setSessions] = useState<WebSessionDto[] | null>(null);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState<CloudError | null>(null);

  const load = useCallback(async () => {
    setBusy(true);
    const result = await commands.accountWebSessions();
    if (!mounted.current) return;
    if (result.status === "ok") {
      setSessions(result.data);
      setError(null);
    } else {
      setError(result.error);
    }
    setBusy(false);
  }, []);

  useEffect(() => {
    mounted.current = true;
    void load();
    return () => { mounted.current = false; };
  }, [load]);

  const nameOf = (session: WebSessionDto) => session.browser ?? t("web_sessions.unknown_browser");

  async function revoke(session: WebSessionDto) {
    if (busy) return;
    if (!await ask({
      title: t("web_sessions.revoke_title", { name: nameOf(session) }),
      description: t("web_sessions.revoke_description"),
      confirmLabel: t("web_sessions.revoke"),
    })) return;
    await apply(commands.accountRevokeWebSession(session.id));
  }

  async function revokeAll() {
    if (busy) return;
    if (!await ask({
      title: t("web_sessions.revoke_all_title"),
      description: t("web_sessions.revoke_all_description"),
      confirmLabel: t("web_sessions.revoke_all_confirm"),
    })) return;
    await apply(commands.accountRevokeAllWebSessions());
  }

  async function apply(pending: ReturnType<typeof commands.accountRevokeAllWebSessions>) {
    setBusy(true);
    const result = await pending;
    if (!mounted.current) return;
    if (result.status === "error") {
      setError(result.error);
      setBusy(false);
      return;
    }
    await load();
  }

  return (
    <div aria-busy={busy}>
      <div className="luna-settings-panel__row">
        <span className="luna-settings-panel__row-title">{t("web_sessions.title")}</span>
        {sessions !== null && sessions.length > 0 ? (
          <button
            type="button"
            className="luna-btn luna-btn--danger-subtle luna-btn--sm"
            disabled={busy}
            onClick={() => void revokeAll()}
          >
            {t("web_sessions.revoke_all")}
          </button>
        ) : <span />}
      </div>
      {sessions === null ? (
        error ? (
          <button type="button" className="luna-btn luna-btn--secondary" onClick={() => void load()}>
            {t("web_sessions.retry")}
          </button>
        ) : <p className="luna-empty" role="status">{t("web_sessions.loading")}</p>
      ) : sessions.length === 0 ? (
        <p className="luna-empty">{t("web_sessions.empty")}</p>
      ) : (
        <ul className="luna-settings-panel__rows">
          {sessions.map((session) => (
            <li key={session.id} className="luna-settings-panel__row">
              <div className="luna-settings-panel__row-label">
                <span className="luna-settings-panel__row-title">{nameOf(session)}</span>
                <p className="luna-settings-panel__row-desc">
                  {[
                    session.os ?? "",
                    t("web_sessions.signed_in", { time: formatInstant(session.created_at, i18n.language) }),
                    session.last_seen_at
                      ? t("web_sessions.last_seen", { time: formatInstant(session.last_seen_at, i18n.language) })
                      : "",
                  ].filter(Boolean).join(" · ")}
                </p>
              </div>
              <button
                type="button"
                className="luna-btn luna-btn--danger-subtle luna-btn--sm"
                disabled={busy}
                onClick={() => void revoke(session)}
              >
                {t("web_sessions.revoke")}
              </button>
            </li>
          ))}
        </ul>
      )}
      {error ? <p className="luna-field-error" role="alert">{errorText(error, t)}</p> : null}
    </div>
  );
}
