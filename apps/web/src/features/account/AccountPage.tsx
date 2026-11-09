/**
 * 「账户与会话」（PRD-21-005 / 006 / 008）：网页访问开关、网页会话列表与逐个 / 全部吊销、空闲锁定时长
 * （只存本浏览器）、退出。吊销到自己（全部吊销、吊销「这个浏览器」那一行、关掉网页访问）成功后当场在本地退出，
 * 回到不带「在别处被吊销」提示的登录页——那是这个浏览器自己的操作。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { messageFor } from "../../lib/error-message";
import { useDialogs } from "../dialog/DialogHost";
import type { SessionSummary, WebSessionView } from "../../worker/protocol";
import { WorkerError, type WorkerClient } from "../../worker/worker-client";
import { IDLE_LOCK_SETTINGS, readIdleLockSetting, writeIdleLockSetting, type IdleLockSetting } from "../session/idle-lock";

export interface AccountPageProps {
  client: WorkerClient;
  session: SessionSummary;
  idleSetting: IdleLockSetting;
  onIdleSettingChange(setting: IdleLockSetting): void;
  onBack(): void;
  onSignOut(): void;
}

function codeOf(error: unknown): string {
  return error instanceof WorkerError ? error.code : "internal";
}

export function AccountPage({ client, session, idleSetting, onIdleSettingChange, onBack, onSignOut }: AccountPageProps) {
  const { t, i18n } = useTranslation("web");
  const dialogs = useDialogs();
  const [webAccess, setWebAccess] = useState<boolean | null>(null);
  const [sessions, setSessions] = useState<{ sessions: WebSessionView[]; max: number } | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const dateFormat = new Intl.DateTimeFormat(i18n.language, { dateStyle: "medium", timeStyle: "short" });

  async function reload() {
    try {
      const [access, listed] = await Promise.all([client.call("get_web_access", {}), client.call("list_sessions", {})]);
      setWebAccess(access.enabled);
      setSessions(listed);
      setProblem(null);
    } catch (error) {
      setProblem(codeOf(error));
    }
  }

  useEffect(() => {
    void reload();
  }, [client]);

  /** `endsThisSession`：这一步连这个浏览器自己的会话也结束了，成功后直接本地退出。 */
  async function run(action: () => Promise<unknown>, endsThisSession = false) {
    setBusy(true);
    try {
      await action();
      if (endsThisSession) {
        onSignOut();
        return;
      }
      await reload();
    } catch (error) {
      setProblem(codeOf(error));
    } finally {
      setBusy(false);
    }
  }

  async function toggleWebAccess(enabled: boolean) {
    if (
      !enabled &&
      !(await dialogs.confirm({
        title: t("account.webAccessConfirm"),
        description: t("account.webAccessHint"),
        confirmLabel: t("account.webAccessConfirmAction"),
      }))
    ) {
      return;
    }
    void run(() => client.call("set_web_access", { enabled }), !enabled);
  }

  return (
    <section className="luna-web-account" aria-labelledby="web-account-title" data-testid="account-page">
      <div className="luna-web-account__head">
        <h2 id="web-account-title" className="luna-web-account__title">
          {t("account.title")}
        </h2>
        <button type="button" className="luna-web-button luna-web-button--quiet" onClick={onBack}>
          {t("account.back")}
        </button>
      </div>
      <p className="luna-web-hint">{t("account.signedInAs", { email: session.account.email })}</p>
      {problem ? (
        <p className="luna-web-error" role="alert">
          {messageFor(t, "error", problem)}
        </p>
      ) : null}

      <div className="luna-web-account__group">
        <label className="luna-web-check">
          <input
            type="checkbox"
            checked={webAccess ?? true}
            disabled={webAccess === null || busy}
            onChange={(event) => void toggleWebAccess(event.target.checked)}
            data-testid="web-access-toggle"
          />
          <span>{t("account.webAccess")}</span>
        </label>
        <p className="luna-web-hint">{t("account.webAccessHint")}</p>
      </div>

      <div className="luna-web-account__group">
        <h3 className="luna-web-account__subtitle">{t("account.sessions")}</h3>
        {sessions ? <p className="luna-web-hint">{t("account.sessionsHint", { max: sessions.max })}</p> : null}
        <ul className="luna-web-sessions" data-testid="session-list">
          {(sessions?.sessions ?? []).map((item) => (
            <li key={item.id} className="luna-web-sessions__item" data-current={item.current || undefined}>
              <span className="luna-web-sessions__name">
                {item.browser && item.os
                  ? t("account.browserOs", { browser: item.browser, os: item.os })
                  : (item.browser ?? item.os ?? t("account.unknownBrowser"))}
                {item.current ? <span className="luna-web-sessions__badge">{t("account.current")}</span> : null}
              </span>
              <span className="luna-web-hint">
                {t("account.lastSeen", { time: dateFormat.format(new Date(item.last_seen_at ?? item.created_at)) })}
                {item.keep_signed_in ? <span className="luna-web-sessions__badge">{t("account.keepSignedIn")}</span> : null}
              </span>
              <button
                type="button"
                className="luna-web-button luna-web-button--quiet"
                disabled={busy}
                onClick={() => void run(() => client.call("revoke_session", { session_id: item.id }), item.current)}
              >
                {t("account.revoke")}
              </button>
            </li>
          ))}
        </ul>
        <button
          type="button"
          className="luna-web-button"
          disabled={busy || !sessions}
          onClick={() => {
            void dialogs
              .confirm({ title: t("account.revokeAllConfirm"), confirmLabel: t("account.revokeAll") })
              .then((confirmed) => {
                if (confirmed) void run(() => client.call("revoke_all_sessions", {}), true);
              });
          }}
        >
          {t("account.revokeAll")}
        </button>
      </div>

      <div className="luna-web-account__group">
        <label className="luna-web-field">
          <span className="luna-web-field__label">{t("account.idle")}</span>
          <select
            className="luna-web-input"
            value={idleSetting}
            data-testid="idle-setting"
            onChange={(event) => {
              const next = event.target.value as IdleLockSetting;
              writeIdleLockSetting(window.localStorage, next);
              onIdleSettingChange(readIdleLockSetting(window.localStorage));
            }}
          >
            {IDLE_LOCK_SETTINGS.map((option) => (
              <option key={option} value={option}>
                {t(`account.idleOption.${option}`)}
              </option>
            ))}
          </select>
        </label>
        <p className="luna-web-hint">{t("account.idleHint")}</p>
      </div>

      <button type="button" className="luna-web-button" onClick={onSignOut}>
        {t("account.signOut")}
      </button>
    </section>
  );
}
