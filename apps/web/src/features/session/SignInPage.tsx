/**
 * 登录页（PRD-21-002 / 004、TDD-17 §4）：邮箱 + 口令（OPAQUE 在 Worker 里做），或整页跳到公共身份源。
 * 没有账户的转到注册页（PRD-21-054）。退出原因（被挤掉 / 被吊销）与身份源失败码在顶部写明。
 */

import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";

export interface SignInPageProps {
  /** 顶部要写明的原因（已经是一句话）。 */
  notice: string | null;
  onSubmit(email: string, password: string, keep: boolean): Promise<string | null>;
  onSignUp(): void;
}

/** 公共身份源的网页流程：整页跳转，`keep` 记在登录事务上（TDD-16 §9）。 */
export function oidcStartUrl(provider: "google" | "facebook", options: { keep?: boolean; unlock?: boolean }): string {
  const query = new URLSearchParams({ provider });
  if (options.keep) query.set("keep", "1");
  if (options.unlock) query.set("unlock", "1");
  return `/api/v1/web/oidc/start?${query.toString()}`;
}

export function SignInPage({ notice, onSubmit, onSignUp }: SignInPageProps) {
  const { t } = useTranslation("web");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [keep, setKeep] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    setError(null);
    const failure = await onSubmit(email, password, keep);
    setPassword("");
    setBusy(false);
    setError(failure);
  }

  return (
    <main className="luna-web-auth" aria-labelledby="web-signin-title">
      <section className="luna-web-auth__card">
        <h1 id="web-signin-title" className="luna-web-auth__title">
          {t("signin.title")}
        </h1>
        <p className="luna-web-auth__subtitle">{t("signin.subtitle")}</p>
        {notice ? (
          <p className="luna-web-auth__notice" role="status" data-testid="signin-notice">
            {notice}
          </p>
        ) : null}
        <form className="luna-web-auth__form" onSubmit={(event) => void submit(event)}>
          <label className="luna-web-field">
            <span className="luna-web-field__label">{t("signin.email")}</span>
            <input
              className="luna-web-input"
              type="email"
              name="email"
              autoComplete="username"
              required
              value={email}
              onChange={(event) => setEmail(event.target.value)}
            />
          </label>
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
          <label className="luna-web-check">
            <input type="checkbox" checked={keep} onChange={(event) => setKeep(event.target.checked)} />
            <span>{t("signin.keep")}</span>
          </label>
          <p className="luna-web-hint">{t("signin.keepHint")}</p>
          {error ? (
            <p className="luna-web-error" role="alert" data-testid="signin-error">
              {error}
            </p>
          ) : null}
          <button className="luna-web-button luna-web-button--primary" type="submit" disabled={busy}>
            {busy ? t("signin.submitting") : t("signin.submit")}
          </button>
        </form>
        <p className="luna-web-auth__or">{t("signin.or")}</p>
        <div className="luna-web-auth__providers">
          <a className="luna-web-button" href={oidcStartUrl("google", { keep })}>
            {t("signin.google")}
          </a>
          <a className="luna-web-button" href={oidcStartUrl("facebook", { keep })}>
            {t("signin.facebook")}
          </a>
        </div>
        <p className="luna-web-auth__signup">
          <span>{t("signin.noAccount")}</span>{" "}
          <button type="button" className="luna-web-auth__switch" onClick={onSignUp}>
            {t("signin.signUp")}
          </button>
        </p>
      </section>
    </main>
  );
}
