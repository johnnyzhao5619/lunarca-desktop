/**
 * 网页应用的会话状态机（TDD-17 §4）：恢复 → 已登录 / 已锁定 / 已退出。秘密全在 Worker 里；锁定与登出
 * 都会终止 Worker（连同 WASM 内存）再起一个新的——主线程从不持有令牌或密钥。
 *
 * 会话在服务端结束（吊销、挤掉）或被别处锁定时，本页还有没存上的改动：先停在「没来得及保存」那一页，
 * 列出是哪几篇、可以复制下来，再去登录 / 解锁（PRD-21-004）。
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { DialogHost } from "../features/dialog/DialogHost";
import type { UnsavedDraft } from "../features/editor/session-registry";
import { LockScreen } from "../features/session/LockScreen";
import { UnsavedNotice } from "../features/session/UnsavedNotice";
import { SignInPage } from "../features/session/SignInPage";
import { SignUpPage } from "../features/session/SignUpPage";
import { SignedOutHeader } from "../features/session/SignedOutHeader";
import { Workspace } from "../features/workspace/Workspace";
import { messageFor } from "../lib/error-message";
import type { SealedDrafts, SessionSummary, WorkerEvent } from "../worker/protocol";
import { spawnWorker } from "../worker/spawn-worker";
import { TERMINATED, WorkerError, type WorkerClient } from "../worker/worker-client";

type Phase =
  | { kind: "booting" }
  | { kind: "signed_out"; notice: string | null }
  | { kind: "locked"; session: SessionSummary }
  | { kind: "ready"; session: SessionSummary; client: WorkerClient }
  /** 会话结束 / 被锁时还有没存上的改动：先提示，再去 `next`。 */
  | { kind: "interrupted"; drafts: UnsavedDraft[]; notice: string | null; next: "signed_out" | "locked" };

/** 公共身份源跳回来时带的失败码（`/?signin_error=`，TDD-17 §4）：读一次就从地址栏抹掉。 */
function takeSigninError(): string | null {
  const url = new URL(window.location.href);
  const code = url.searchParams.get("signin_error");
  if (code === null) return null;
  url.searchParams.delete("signin_error");
  window.history.replaceState(null, "", `${url.pathname}${url.search}${url.hash}`);
  return /^[a-z][a-z0-9_]*$/.test(code) ? code : "unknown";
}

function codeOf(error: unknown): string {
  return error instanceof WorkerError ? error.code : "internal";
}

export interface AppProps {
  /** 起 Worker 的方式（测试换成假的）。 */
  spawn?: () => WorkerClient;
}

export function App({ spawn = spawnWorker }: AppProps) {
  const { t, i18n } = useTranslation("web");
  const [phase, setPhase] = useState<Phase>({ kind: "booting" });
  const [signingUp, setSigningUp] = useState(false);
  const worker = useRef<WorkerClient | null>(null);
  const unsavedProbe = useRef<(() => UnsavedDraft[]) | null>(null);
  // 离线锁定时封存的改动：只在这一页的内存里，密文；解锁后由 Workspace 解开接着存（TDD-17 §4）。
  const sealed = useRef<SealedDrafts | null>(null);
  // 断网时服务端没记上锁定：联网就补上，否则刷新接口还开着。
  const lockPending = useRef(false);

  /** 会话没了：还有没存上的改动就先停在提示页（在 Worker 被换掉之前问，问完再换）。 */
  function interrupted(next: "signed_out" | "locked", notice: string | null): boolean {
    const drafts = unsavedProbe.current?.() ?? [];
    if (drafts.length === 0) return false;
    unsavedProbe.current = null;
    setPhase({ kind: "interrupted", drafts, notice, next });
    return true;
  }

  /** 换一个全新的 Worker：旧的连同它的令牌、密钥环与清单一起终止。 */
  function fresh(): WorkerClient {
    worker.current?.terminate();
    const next = spawn();
    worker.current = next;
    next.onEvent((event: WorkerEvent) => {
      if (worker.current !== next) return;
      if (event.event === "session_ended") {
        const notice = event.reason ? messageFor(t, "ended", event.reason) : null;
        if (interrupted("signed_out", notice)) {
          fresh();
          return;
        }
        fresh();
        setPhase({ kind: "signed_out", notice });
      } else if (event.event === "session_locked") {
        if (interrupted("locked", null)) {
          fresh();
          return;
        }
        void enterLock();
      }
    });
    return next;
  }

  /**
   * 进锁屏：新 Worker 只问服务端这个会话是谁、能怎么解锁。`known` 是页面自己锁定时手上的账户信息：
   * 断网问不到，或服务端没记上锁定，也照样进锁屏，等联网补上。
   */
  async function enterLock(known?: SessionSummary): Promise<void> {
    const next = fresh();
    const offlineLock = (session: SessionSummary) => {
      lockPending.current = true;
      setPhase({ kind: "locked", session });
    };
    try {
      const info = await next.call("info", {});
      if (info.locked) setPhase({ kind: "locked", session: info.session });
      else if (known) offlineLock(known);
      else setPhase({ kind: "signed_out", notice: null });
    } catch (error) {
      if (codeOf(error) === TERMINATED) return;
      if (known) offlineLock(known);
      else setPhase({ kind: "signed_out", notice: null });
    }
  }

  /** 把断网时没记上的锁定补给服务端。 */
  async function settleLock(): Promise<void> {
    if (!lockPending.current) return;
    try {
      await worker.current?.call("lock", {});
      lockPending.current = false;
    } catch {
      // 还连不上：下次联网再补。
    }
  }

  function ready(session: SessionSummary): void {
    if (worker.current) setPhase({ kind: "ready", session, client: worker.current });
  }

  async function signIn(email: string, password: string, keep: boolean): Promise<string | null> {
    try {
      ready(await (worker.current ?? fresh()).call("login_password", { email, password, keep }));
      return null;
    } catch (error) {
      return messageFor(t, "error", codeOf(error));
    }
  }

  /** 注册的一步：失败回一句话（验证码不对单独说，别说成「邮箱或口令不对」）。 */
  async function signupStep(run: () => Promise<unknown>): Promise<string | null> {
    try {
      await run();
      return null;
    } catch (error) {
      const code = codeOf(error);
      return code === "invalid_credentials" ? t("signup.badCode") : messageFor(t, "error", code);
    }
  }

  function signupClient() {
    return worker.current ?? fresh();
  }

  async function unlock(password: string): Promise<string | null> {
    try {
      await settleLock();
      ready(await (worker.current ?? fresh()).call("unlock_password", { password }));
      return null;
    } catch (error) {
      return messageFor(t, "error", codeOf(error));
    }
  }

  async function lock(session: SessionSummary): Promise<void> {
    const drafts = unsavedProbe.current?.() ?? [];
    if (drafts.length > 0) {
      try {
        sealed.current = await worker.current?.call("seal_drafts", { drafts }) ?? null;
      } catch {
        // 封存不了（没有打开的库）：退回提示页，让用户把内容复制走。
        if (interrupted("locked", null)) {
          fresh();
          return;
        }
      }
    }
    try {
      await worker.current?.call("lock", {});
    } catch {
      // 服务端没记上锁定（断网）：这边照样清零进锁屏，联网后补上。
    }
    await enterLock(session);
  }

  async function signOut(): Promise<void> {
    try {
      await worker.current?.call("logout", {});
    } catch {
      // 登出接口恒 204；失败（断网）也在本地清零。
    }
    sealed.current = null;
    lockPending.current = false;
    fresh();
    setPhase({ kind: "signed_out", notice: null });
  }

  useEffect(() => {
    if (phase.kind !== "locked") return;
    const retry = () => void settleLock();
    window.addEventListener("online", retry);
    return () => window.removeEventListener("online", retry);
  }, [phase.kind]);

  useEffect(() => {
    // 封存的改动只在这一页的内存里：关页前拦一下。
    const guard = (event: BeforeUnloadEvent) => {
      if (sealed.current) event.preventDefault();
    };
    window.addEventListener("beforeunload", guard);
    return () => window.removeEventListener("beforeunload", guard);
  }, []);

  useEffect(() => {
    const signinError = takeSigninError();
    void fresh()
      .call("restore", {})
      .then(
        (outcome) => {
          if (outcome.state === "signed_in") ready(outcome.session);
          else if (outcome.state === "locked") setPhase({ kind: "locked", session: outcome.session });
          else {
            const notice = signinError
              ? messageFor(t, "signinError", signinError)
              : outcome.reason
                ? messageFor(t, "ended", outcome.reason)
                : null;
            setPhase({ kind: "signed_out", notice });
          }
        },
        (error: unknown) => {
          if (codeOf(error) !== TERMINATED) setPhase({ kind: "signed_out", notice: messageFor(t, "error", codeOf(error)) });
        },
      );
    return () => {
      worker.current?.terminate();
      worker.current = null;
    };
    // 只在挂载时恢复一次会话。
  }, []);

  const screen = (() => {
    switch (phase.kind) {
      case "booting":
        return (
          <main className="luna-web-auth" aria-busy="true">
            <p className="luna-web-hint">{t("app.connecting")}</p>
          </main>
        );
      case "signed_out":
        return signingUp ? (
          <SignUpPage
            onBack={() => setSigningUp(false)}
            onStart={(email) => signupStep(() => signupClient().call("signup_start", { email, locale: i18n.language }))}
            onVerify={(email, code) => signupStep(() => signupClient().call("signup_verify", { email, code }))}
            onComplete={(password, keep, termsVersion) =>
              signupStep(async () => {
                ready(await signupClient().call("signup_complete", { password, keep, terms_version: termsVersion }));
                setSigningUp(false);
              })
            }
          />
        ) : (
          <SignInPage notice={phase.notice} onSubmit={signIn} onSignUp={() => setSigningUp(true)} />
        );
      case "locked":
        return (
          <LockScreen
            session={phase.session}
            onUnlock={unlock}
            onSignOut={() => void signOut()}
            sealedCount={sealed.current?.count ?? 0}
          />
        );
      case "ready":
        return (
          <Workspace
            client={phase.client}
            session={phase.session}
            onLock={() => void lock(phase.session)}
            onSignOut={() => void signOut()}
            unsavedProbe={unsavedProbe}
            sealed={sealed.current}
            onSealedRestored={() => {
              sealed.current = null;
            }}
          />
        );
      case "interrupted":
        return (
          <UnsavedNotice
            drafts={phase.drafts}
            locked={phase.next === "locked"}
            onContinue={() => {
              if (phase.next === "locked") void enterLock();
              else setPhase({ kind: "signed_out", notice: phase.notice });
            }}
          />
        );
    }
  })();

  const signedOut = phase.kind === "signed_out" || phase.kind === "booting" ||
    (phase.kind === "interrupted" && phase.next === "signed_out");
  return (
    <DialogHost>
      {signedOut ? (
        <div className="luna-web-shell">
          <SignedOutHeader />
          {screen}
        </div>
      ) : screen}
    </DialogHost>
  );
}
