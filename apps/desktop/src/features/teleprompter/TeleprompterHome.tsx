/**
 * 提词器首页（ADR-176 ②，PRD-20-004 / 005）：活动栏「提词器」那一格。
 *
 * 开始提词（继续当前一篇，没有就打开最近一篇）、最近提词过的稿子、新建提词稿 / 从剪贴板新建，
 * 以及跟读总开关（默认关）。最近列表、当前稿与提词稿目录都由 Rust 给出（`teleprompter_home`），
 * 本页只渲染与发命令（ADR-003）。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { commands, type TeleprompterHomeDto } from "../../ipc/bindings";
import { EmptyState } from "../../components/ui/EmptyState";
import { ScrollTextIcon } from "@lunarca/editor-core/Icon";
import { useVaultReload } from "../../hooks/use-vault-reload";
import {
  TELEPROMPTER_DROP_ATTR,
  TELEPROMPTER_FILE_DROP_EVENT,
  type TeleprompterFileDrop,
} from "../../lib/teleprompter-file-drop";
import { vaultErrorText } from "../../lib/vault-error";
import { createTeleprompterDraft, importTeleprompterFiles, openInTeleprompter } from "./teleprompter-actions";
import { useTeleprompterPrefs } from "./use-teleprompter-prefs";

/** 粘贴事件落在可编辑的东西里（输入框、编辑器）时，那是它自己的粘贴，首页不接。 */
function pastedIntoEditable(event: ClipboardEvent): boolean {
  const target = event.target;
  return (
    target instanceof HTMLElement &&
    (target.isContentEditable || target.closest("input, textarea, [contenteditable='true']") !== null)
  );
}

export function TeleprompterHome() {
  const { t } = useTranslation("workspace");
  const prefs = useTeleprompterPrefs();
  const [home, setHome] = useState<TeleprompterHomeDto | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    const reply = await commands.teleprompterHome();
    if (reply.status === "ok") {
      setHome(reply.data);
    } else {
      setError(vaultErrorText(reply.error));
    }
  }, []);

  useEffect(() => {
    void reload();
    window.addEventListener("focus", reload);
    return () => window.removeEventListener("focus", reload);
  }, [reload]);
  useVaultReload(() => void reload());

  const run = useCallback(
    async (action: () => Promise<string | null>) => {
      setError(null);
      const failure = await action();
      if (failure) setError(failure);
      void reload();
    },
    [reload],
  );

  // ⌘V 即提词（PRD-20-020）：首页上任何地方粘贴（不在输入框里）就把那段文字存成提词稿并打开。
  useEffect(() => {
    const onPaste = (event: ClipboardEvent) => {
      if (pastedIntoEditable(event)) return;
      const text = event.clipboardData?.getData("text/plain") ?? "";
      if (!text.trim()) return;
      event.preventDefault();
      void run(() => createTeleprompterDraft({ kind: "text", text }));
    };
    document.addEventListener("paste", onPaste);
    return () => document.removeEventListener("paste", onPaste);
  }, [run]);

  // 拖文件到首页（PRD-20-022）：主窗口的拖放监听把落在这里的文件交过来。
  const sectionRef = useRef<HTMLElement | null>(null);
  useEffect(() => {
    const section = sectionRef.current;
    if (!section) return;
    const onDrop = (event: Event) => {
      const { paths } = (event as CustomEvent<TeleprompterFileDrop>).detail;
      void run(() => importTeleprompterFiles(paths));
    };
    section.addEventListener(TELEPROMPTER_FILE_DROP_EVENT, onDrop);
    return () => section.removeEventListener(TELEPROMPTER_FILE_DROP_EVENT, onDrop);
  }, [run]);

  const startPath = home?.current ?? home?.recent[0]?.path ?? null;
  // 当前稿打开时就记进了最近列表（Rust 一处），标题取那里的，不在前端由路径推。
  const currentTitle = home?.current ? home.recent.find((item) => item.path === home.current)?.title : undefined;
  const followId = "luna-teleprompter-home-follow";

  return (
    <section
      ref={sectionRef}
      className="luna-teleprompter-home"
      aria-labelledby="luna-teleprompter-home-title"
      {...{ [TELEPROMPTER_DROP_ATTR]: "true" }}
    >
      <header className="luna-main-header">
        <div className="luna-main-header__title">
          <h1 id="luna-teleprompter-home-title">{t("teleprompter.home.title")}</h1>
        </div>
        <div className="luna-main-header__actions">
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            disabled={!home}
            onClick={() => void run(() => createTeleprompterDraft({ kind: "clipboard" }))}
          >
            {t("teleprompter.home.fromClipboard")}
          </button>
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            disabled={!home}
            onClick={() => void run(() => createTeleprompterDraft({ kind: "empty" }))}
          >
            {t("teleprompter.home.newDraft")}
          </button>
          <button
            type="button"
            className="luna-btn luna-btn--primary"
            disabled={!startPath}
            onClick={() => startPath && void run(() => openInTeleprompter(startPath))}
          >
            {currentTitle ? t("teleprompter.home.startCurrent", { title: currentTitle }) : t("teleprompter.home.start")}
          </button>
        </div>
      </header>

      <p className="luna-teleprompter-home__lede">{t("teleprompter.home.lede")}</p>
      <p className="luna-teleprompter-home__hint">{t("teleprompter.home.sendHint")}</p>
      {error ? (
        <p className="luna-teleprompter-home__error" role="alert">
          {error}
        </p>
      ) : null}

      <section className="luna-teleprompter-home__section" aria-labelledby="luna-teleprompter-home-recent">
        <h2 id="luna-teleprompter-home-recent">{t("teleprompter.home.recent")}</h2>
        {home === null ? (
          <p className="luna-teleprompter-home__hint">{t("teleprompter.home.loading")}</p>
        ) : home.recent.length === 0 ? (
          <EmptyState
            icon={<ScrollTextIcon />}
            title={t("teleprompter.home.recentEmpty")}
            description={t("teleprompter.home.startHint")}
          />
        ) : (
          <ul className="luna-teleprompter-home__list">
            {home.recent.map((item) => (
              <li key={item.path} className="luna-teleprompter-home__row">
                <span className="luna-teleprompter-home__name" title={item.path}>
                  {item.title}
                </span>
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary luna-btn--sm"
                  onClick={() => void run(() => openInTeleprompter(item.path))}
                >
                  {t("teleprompter.home.open")}
                </button>
              </li>
            ))}
          </ul>
        )}
        {home ? (
          <p className="luna-teleprompter-home__hint">{t("teleprompter.home.draftHint", { dir: home.draft_dir })}</p>
        ) : null}
      </section>

      <section className="luna-teleprompter-home__section" aria-labelledby="luna-teleprompter-home-settings">
        <h2 id="luna-teleprompter-home-settings">{t("teleprompter.home.settings")}</h2>
        <div className="luna-teleprompter-home__row">
          <div className="luna-teleprompter-home__setting">
            <span id={followId} className="luna-teleprompter-home__name">
              {t("teleprompter.home.follow")}
            </span>
            <span className="luna-teleprompter-home__hint">{t("teleprompter.home.followDesc")}</span>
          </div>
          <button
            type="button"
            role="switch"
            aria-checked={prefs.followEnabled}
            aria-labelledby={followId}
            className="luna-switch"
            onClick={() => void prefs.setFollowEnabled(!prefs.followEnabled)}
          >
            <span className="luna-switch__track" aria-hidden="true">
              <span className="luna-switch__thumb" />
            </span>
          </button>
        </div>
      </section>
    </section>
  );
}
