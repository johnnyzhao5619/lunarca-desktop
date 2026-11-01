/**
 * 关于与更新（PRD-09-051，TDD-13 §5；客户端分发 D2）：当前版本、检查更新、发布说明、下载并安装。
 *
 * 判定全在 Rust：严格本地拦不拦、验签、下载安装与重启、提问卡显不显示都不在前端（ADR-003）。这里只发
 * 命令、按返回的结果与稳定错误码显示文字。检查更新是 opt-in 联网：缺省仅手动，提问卡问一次要不要
 * 启动时检查；之后由设置「应用更新」决定（同页「启动与通知」分节）。
 */
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { commands, events, type AppReleaseDto } from "../../ipc/bindings";

/** Rust 侧会回的稳定错误码（按小写比对）；不在其中的一律显示「没有完成」。 */
export const RELEASE_ERROR_CODES = [
  "strict_local_blocked",
  "update_unavailable",
  "update_check_failed",
  "no_pending_update",
  "update_install_failed",
] as const;

type ReleaseErrorCode = (typeof RELEASE_ERROR_CODES)[number] | "unknown";

type Phase =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "latest" }
  | { kind: "available"; release: AppReleaseDto }
  | { kind: "installing"; release: AppReleaseDto }
  | { kind: "error"; code: ReleaseErrorCode; release: AppReleaseDto | null };

export function releaseErrorCode(raw: unknown): ReleaseErrorCode {
  const code = String(raw).toLowerCase();
  return (RELEASE_ERROR_CODES as readonly string[]).includes(code)
    ? (code as ReleaseErrorCode)
    : "unknown";
}

export function AppReleasePanel() {
  const { t } = useTranslation("settings");
  const [version, setVersion] = useState<string | null>(null);
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  const [askStartupCheck, setAskStartupCheck] = useState(false);

  const refreshPrompt = useCallback(() => {
    void commands
      .appReleaseStartupCheckPrompt()
      .then(setAskStartupCheck)
      .catch(() => {
        // 同 appInfo：IPC 不可用时不显示提问卡。
      });
  }, []);

  useEffect(() => {
    refreshPrompt();
    // 用户在下拉里自己改了策略、或打开了严格本地，提问卡要随之消失。
    const unlisten = events.settingsChanged.listen((event) => {
      const key = event.payload.key;
      if (key === "update.policy" || key === "update.startup_check_prompt" || key === "privacy.strict_local") {
        refreshPrompt();
      }
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [refreshPrompt]);

  useEffect(() => {
    let disposed = false;
    void commands
      .appInfo()
      .then((info) => {
        if (!disposed) setVersion(info.version);
      })
      .catch(() => {
        // 应用正在退出时 IPC 可能已不可用；只读信息取不到就不显示版本号。
      });
    return () => {
      disposed = true;
    };
  }, []);

  const check = async () => {
    setPhase({ kind: "checking" });
    const result = await commands.appReleaseCheck();
    if (result.status === "error") {
      setPhase({ kind: "error", code: releaseErrorCode(result.error), release: null });
    } else if (result.data === null) {
      setPhase({ kind: "latest" });
    } else {
      setPhase({ kind: "available", release: result.data });
    }
  };

  const install = async (release: AppReleaseDto) => {
    setPhase({ kind: "installing", release });
    // 成功时 Rust 装好就重启应用，这里等不到返回；只有失败会回来。
    const result = await commands.appReleaseInstall();
    if (result.status === "error") {
      setPhase({ kind: "error", code: releaseErrorCode(result.error), release });
    }
  };

  const answer = async (enable: boolean) => {
    const result = await commands.appReleaseAnswerStartupCheck(enable);
    if (result.status === "ok") refreshPrompt();
  };

  const busy = phase.kind === "checking" || phase.kind === "installing";
  const release = phase.kind === "available" || phase.kind === "installing" ? phase.release : null;

  return (
    <section className="luna-settings-panel" aria-labelledby="app-release-title">
      <header className="luna-settings-panel__header">
        <div>
          <h2 id="app-release-title" className="luna-settings-panel__section-title">
            {t("app_release.title")}
          </h2>
          <p className="luna-settings-panel__desc">{t("app_release.description")}</p>
        </div>
      </header>
      {version === null ? null : (
        <p className="luna-settings-panel__desc">{t("app_release.version", { version })}</p>
      )}
      {askStartupCheck ? (
        <div className="luna-settings-panel__desc" role="group" aria-labelledby="app-release-ask-title">
          <p id="app-release-ask-title">{t("app_release.ask_title")}</p>
          <p>{t("app_release.ask_body")}</p>
          <div className="luna-settings-panel__actions">
            <button type="button" className="luna-btn luna-btn--primary" onClick={() => void answer(true)}>
              {t("app_release.ask_enable")}
            </button>
            <button type="button" className="luna-btn luna-btn--secondary" onClick={() => void answer(false)}>
              {t("app_release.ask_decline")}
            </button>
          </div>
        </div>
      ) : null}
      {release === null ? null : (
        <div className="luna-settings-panel__desc">
          <p>{t("app_release.available", { version: release.version })}</p>
          {release.notes ? (
            <>
              <p>{t("app_release.notes")}</p>
              {release.notes.split("\n").filter((line) => line.trim() !== "").map((line, index) => (
                <p key={index}>{line}</p>
              ))}
            </>
          ) : null}
        </div>
      )}
      <div className="luna-settings-panel__actions">
        {release !== null ? (
          <button
            type="button"
            className="luna-btn luna-btn--primary"
            disabled={busy}
            onClick={() => void install(release)}
          >
            {phase.kind === "installing" ? t("app_release.installing") : t("app_release.install")}
          </button>
        ) : (
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            disabled={busy}
            onClick={() => void check()}
          >
            {phase.kind === "checking" ? t("app_release.checking") : t("app_release.check")}
          </button>
        )}
        {phase.kind === "latest" ? (
          <span className="luna-settings-panel__hint" role="status">
            {t("app_release.up_to_date")}
          </span>
        ) : null}
        {phase.kind === "error" ? (
          <span className="luna-settings-panel__hint" role="alert">
            {t(`app_release.error.${phase.code}`)}
          </span>
        ) : null}
      </div>
    </section>
  );
}
