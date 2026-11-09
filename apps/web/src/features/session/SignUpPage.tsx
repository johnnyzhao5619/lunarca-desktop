/**
 * 注册页（PRD-21-054、ADR-178 ⑤）：邮箱 → 验证码 → 设口令并勾选条款。口令和验证码都只在 Worker 里进
 * OPAQUE，完成后直接进入（第一次进入会建默认库，PRD-21-003）。
 */

import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";

/** 注册时同意的条款版本，随官网条款改版一起改；服务端记进审计。 */
const TERMS_VERSION = "terms-1.3/privacy-1.4";
const MIN_PASSWORD_LENGTH = 8;

export interface SignUpPageProps {
  onBack(): void;
  /** 三步各一个回调；失败回一句话，成功回 `null`。 */
  onStart(email: string): Promise<string | null>;
  onVerify(email: string, code: string): Promise<string | null>;
  onComplete(password: string, keep: boolean, termsVersion: string): Promise<string | null>;
}

type Step = "email" | "code" | "password";

export function SignUpPage({ onBack, onStart, onVerify, onComplete }: SignUpPageProps) {
  const { t } = useTranslation("web");
  const [step, setStep] = useState<Step>("email");
  const [email, setEmail] = useState("");
  const [code, setCode] = useState("");
  const [password, setPassword] = useState("");
  const [keep, setKeep] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function run(action: () => Promise<string | null>, next?: Step) {
    if (busy) return;
    setBusy(true);
    setError(null);
    const failure = await action();
    setBusy(false);
    setError(failure);
    if (failure === null && next) setStep(next);
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    if (step === "email") void run(() => onStart(email), "code");
    else if (step === "code") void run(() => onVerify(email, code), "password");
    else void run(() => onComplete(password, keep, TERMS_VERSION));
  }

  return (
    <main className="luna-web-auth" aria-labelledby="web-signup-title">
      <section className="luna-web-auth__card">
        <h1 id="web-signup-title" className="luna-web-auth__title">
          {t("signup.title")}
        </h1>
        <p className="luna-web-auth__subtitle">{t("signup.subtitle")}</p>
        <form className="luna-web-auth__form" onSubmit={submit}>
          {step === "email" ? (
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
          ) : null}
          {step === "code" ? (
            <>
              <p className="luna-web-hint" role="status">
                {t("signup.codeSent", { email })}
              </p>
              <label className="luna-web-field">
                <span className="luna-web-field__label">{t("signup.code")}</span>
                <input
                  className="luna-web-input"
                  name="one-time-code"
                  autoComplete="one-time-code"
                  required
                  value={code}
                  onChange={(event) => setCode(event.target.value)}
                />
              </label>
            </>
          ) : null}
          {step === "password" ? (
            <>
              <label className="luna-web-field">
                <span className="luna-web-field__label">{t("signup.password")}</span>
                <input
                  className="luna-web-input"
                  type="password"
                  name="new-password"
                  autoComplete="new-password"
                  required
                  minLength={MIN_PASSWORD_LENGTH}
                  value={password}
                  onChange={(event) => setPassword(event.target.value)}
                />
              </label>
              <p className="luna-web-hint">{t("signup.passwordHint", { count: MIN_PASSWORD_LENGTH })}</p>
              <label className="luna-web-check">
                <input type="checkbox" checked={keep} onChange={(event) => setKeep(event.target.checked)} />
                <span>{t("signin.keep")}</span>
              </label>
              <p className="luna-web-hint">
                {t("signup.agree")}{" "}
                <a href={t("footer.termsUrl")} target="_blank" rel="noopener noreferrer">
                  {t("footer.terms")}
                </a>{" "}
                {t("signup.and")}{" "}
                <a href={t("footer.privacyUrl")} target="_blank" rel="noopener noreferrer">
                  {t("footer.privacy")}
                </a>
              </p>
            </>
          ) : null}
          {error ? (
            <p className="luna-web-error" role="alert" data-testid="signup-error">
              {error}
            </p>
          ) : null}
          <button type="submit" className="luna-web-button luna-web-button--primary" disabled={busy}>
            {t(busy ? "signup.working" : step === "email" ? "signup.sendCode" : step === "code" ? "signup.verify" : "signup.create")}
          </button>
        </form>
        <p className="luna-web-auth__signup">
          <span>{t("signup.haveAccount")}</span>{" "}
          <button type="button" className="luna-web-auth__switch" onClick={onBack}>
            {t("signup.backToSignIn")}
          </button>
        </p>
      </section>
    </main>
  );
}
