/**
 * 锁屏（PRD-21-008、TDD-17 §4）：会话在服务端已锁，屏幕上不留正文。按 `session/info` 给出的方式解锁——
 * 有口令就输口令（邮箱由服务端定），关联了公共身份源就整页跳去重走一次；都在原会话上继续。
 */

import { useState, type FormEvent } from "react";
import { useDialogs } from "../dialog/DialogHost";
import { useTranslation } from "react-i18next";
import type { SessionSummary } from "../../worker/protocol";
import { oidcStartUrl } from "./SignInPage";

export interface LockScreenProps {
  session: SessionSummary;
  onUnlock(password: string): Promise<string | null>;
  onSignOut(): void;
  /** 离线锁定时封存下来、等解锁后接着存的篇数（只在这一页的内存里）。 */
  sealedCount: number;
}

const PROVIDERS = ["google", "facebook"] as const;

export function LockScreen({ session, onUnlock, onSignOut, sealedCount }: LockScreenProps) {
  const { t } = useTranslation("web");
  const dialogs = useDialogs();
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const providers = PROVIDERS.filter((provider) => session.account.providers.includes(provider));

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    setError(null);
    const failure = await onUnlock(password);
    setPassword("");
    setBusy(false);
    setError(failure);
  }

  async function signOut() {
    if (sealedCount > 0) {
      const confirmed = await dialogs.confirm({
        title: t("note.signOutUnsaved.title"),
        description: t("note.signOutUnsaved.body", { count: sealedCount }),
        confirmLabel: t("note.signOutUnsaved.confirm"),
      });
      if (!confirmed) return;
    }
    onSignOut();
  }

  return (
    <main className="luna-web-auth" aria-labelledby="web-lock-title" data-testid="lock-screen">
      <section className="luna-web-auth__card">
        <h1 id="web-lock-title" className="luna-web-auth__title">
          {t("lock.title")}
        </h1>
        <p className="luna-web-auth__subtitle">{t("lock.body")}</p>
        <p className="luna-web-auth__account">{t("lock.account", { email: session.account.email })}</p>
        {sealedCount > 0 ? (
          <p className="luna-web-hint" data-testid="lock-sealed">{t("lock.sealed", { count: sealedCount })}</p>
        ) : null}
        {session.account.has_password ? (
          <form className="luna-web-auth__form" onSubmit={(event) => void submit(event)}>
            <input type="email" name="email" autoComplete="username" value={session.account.email} readOnly hidden />
            <label className="luna-web-field">
              <span className="luna-web-field__label">{t("signin.password")}</span>
              <input
                className="luna-web-input"
                type="password"
                name="password"
                autoComplete="current-password"
                required
                value={password}
                onChange={(event) => setPassword(event.target.value)}
              />
            </label>
            {error ? (
              <p className="luna-web-error" role="alert" data-testid="unlock-error">
                {error}
              </p>
            ) : null}
            <button className="luna-web-button luna-web-button--primary" type="submit" disabled={busy}>
              {busy ? t("lock.unlocking") : t("lock.unlock")}
            </button>
          </form>
        ) : null}
        {providers.length > 0 ? (
          <div className="luna-web-auth__providers">
            {providers.map((provider) => (
              <a key={provider} className="luna-web-button" href={oidcStartUrl(provider, { unlock: true })}>
                {provider === "google" ? t("lock.google") : t("lock.facebook")}
              </a>
            ))}
          </div>
        ) : null}
        <button className="luna-web-button luna-web-button--quiet" type="button" onClick={() => void signOut()}>
          {t("lock.signOut")}
        </button>
      </section>
    </main>
  );
}
