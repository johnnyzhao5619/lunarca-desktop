/**
 * 「应用 › 主题」页签（ADR-168 ⑦，离线先行）：内置主题与从文件导入的主题包（可以同时装多套，
 * ADR-169 ④），在这里预览、使用、从文件安装与卸载。下方「获取更多主题」列在线主题目录（纵切 7a，
 * `useThemeMarket`）：严格本地、未登录或服务端没发布主题目录时 Rust 回 `null`，这一块整个不出现——不放空入口。
 *
 * 判定都在 Rust：导入走内容基座的安装管线（解析、封闭 Token 与对比度校验、同 id 比版本，
 * `install_theme_package`），有哪些主题、在用哪一套是主题注册表的投影（`useThemeLibrary`），卸载正在用的
 * 那套先切回内置（`uninstall_theme_package`）。这里只画卡片、接按钮。**预览不写设置**：只换根节点
 * 的族，离开这一页或点「结束预览」就原样放回。设置 › 外观里的主题库只留「选用哪一套」，并链到这里。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { commands } from "../../ipc/bindings";
import { TaskStatus } from "../../components/TaskStatus";
import { ThemeSwatch } from "../../components/ThemeSwatch";
import { endThemePreview, previewThemeFamily } from "../../theme/theme";
import { useThemeLibrary, type ThemeCard } from "../../theme/use-theme-library";
import { useThemeMarket } from "../../theme/use-theme-market";
import type { MarketThemeDto } from "../../ipc/bindings";
import { importNotice, ThemeNoticeLine, useThemeActions } from "./use-theme-actions";

const THEME_FAMILY_KEY = "appearance.theme_family";

/** 正在预览的那一套：本机的（可以直接「使用」），还是在线目录里的（要先获取）。 */
interface Preview {
  id: string;
  name: string;
  market: MarketThemeDto | null;
}

export function ThemesTab() {
  const { t } = useTranslation("workspace");
  const library = useThemeLibrary();
  const { market, reload: reloadMarket } = useThemeMarket();
  const [previewing, setPreviewing] = useState<Preview | null>(null);
  const [busy, setBusy] = useState(false);

  // 离开这一页就结束预览：预览从不落盘，不能让它跟着人走到别的页面。
  useEffect(() => () => endThemePreview(), []);

  const entries = library.themes;

  const stopPreview = () => {
    endThemePreview();
    setPreviewing(null);
  };

  const actions = useThemeActions(library, { beforeChange: stopPreview, onSettled: reloadMarket });
  const { getting, setNotice } = actions;

  const tryOn = (entry: ThemeCard) => {
    previewThemeFamily(entry.id, entry.css);
    setPreviewing({ id: entry.id, name: entry.displayName, market: null });
  };

  const tryOnMarket = (theme: MarketThemeDto) => {
    previewThemeFamily(theme.id, theme.css);
    setPreviewing({ id: theme.id, name: theme.name, market: theme });
  };

  /** 获取或更新一套在线主题：第一次获取即启用（与从文件安装一致），更新不改在用的那一套。 */
  const getTheme = (theme: MarketThemeDto) => actions.get(theme.id, theme.state === "update_available");

  const use = async (id: string) => {
    stopPreview();
    setNotice(null);
    const result = await commands.setSetting(THEME_FAMILY_KEY, { kind: "str", value: id });
    if (result.status !== "ok") setNotice({ key: "apps.themes.saveFailed" });
  };

  const installFromFile = async () => {
    stopPreview();
    setBusy(true);
    setNotice(null);
    const result = await commands.installThemePackage();
    setBusy(false);
    if (result.status === "error") {
      setNotice(importNotice(result.error));
      return;
    }
    // `null` = 用户在选文件时取消了：什么都没发生，不说话。
    if (result.data) {
      library.reload();
      setNotice({ key: "apps.themes.installed", name: result.data.name });
    }
  };

  return (
    <div className="luna-apps__panel">
      <div className="luna-apps__updates">
        <p className="luna-apps__lede">{t("apps.themes.lede")}</p>
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          disabled={busy}
          onClick={() => void installFromFile()}
        >
          {busy ? t("apps.themes.installing") : t("apps.themes.installFromFile")}
        </button>
      </div>
      <ThemeNoticeLine notice={actions.notice} />
      {previewing !== null ? (
        <div className="luna-apps__preview" role="status">
          <span>{t("apps.themes.previewing", { name: previewing.name })}</span>
          <span className="luna-apps__row-side">
            <button type="button" className="luna-btn luna-btn--secondary" onClick={stopPreview}>
              {t("apps.themes.stopPreview")}
            </button>
            {previewing.market === null ? (
              <button type="button" className="luna-btn luna-btn--primary" onClick={() => void use(previewing.id)}>
                {t("apps.themes.use")}
              </button>
            ) : previewing.market.state === "available" || previewing.market.state === "update_available" ? (
              <button
                type="button"
                className="luna-btn luna-btn--primary"
                disabled={getting !== null}
                onClick={() => previewing.market && void getTheme(previewing.market)}
              >
                {previewing.market.state === "available"
                  ? t("apps.themes.market.getAndUse")
                  : t("apps.themes.market.update")}
              </button>
            ) : null}
          </span>
        </div>
      ) : null}
      <ul className="luna-apps__list" aria-label={t("apps.tab.themes")}>
        {entries.map((entry) => (
          <li key={entry.id} className="luna-apps__row">
            <span className="luna-apps__row-lead">
              <ThemeSwatch id={entry.id} />
              <span className="luna-apps__card-text">
                <span className="luna-apps__card-name">{entry.displayName}</span>
                <span className="luna-apps__meta">
                  {entry.source === "builtin"
                    ? t("apps.themes.builtin")
                    : entry.update_to !== null
                      ? t("apps.themes.localMetaUpdate", {
                          author: entry.author,
                          license: entry.license,
                          version: entry.version,
                          to: entry.update_to,
                        })
                      : t("apps.themes.localMeta", { author: entry.author, license: entry.license, version: entry.version })}
                </span>
              </span>
            </span>
            <span className="luna-apps__row-side">
              {library.inUse === entry.id ? (
                <TaskStatus kind="completed" label={t("apps.themes.inUse")} />
              ) : (
                <>
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary"
                    aria-label={t("apps.rowAction", { action: t("apps.themes.tryOn"), name: entry.displayName })}
                    disabled={previewing?.id === entry.id}
                    onClick={() => tryOn(entry)}
                  >
                    {t("apps.themes.tryOn")}
                  </button>
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary"
                    aria-label={t("apps.rowAction", { action: t("apps.themes.use"), name: entry.displayName })}
                    onClick={() => void use(entry.id)}
                  >
                    {t("apps.themes.use")}
                  </button>
                </>
              )}
              {entry.source === "content" ? (
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary"
                  aria-label={t("apps.rowAction", { action: t("apps.themes.uninstall.action"), name: entry.displayName })}
                  onClick={() => void actions.uninstall(entry.id, entry.displayName, library.inUse === entry.id)}
                >
                  {t("apps.themes.uninstall.action")}
                </button>
              ) : null}
            </span>
          </li>
        ))}
      </ul>
      <p className="luna-apps__note">{t("apps.themes.formatHint")}</p>
      {library.disabled.length > 0 ? (
        <section className="luna-apps__section" aria-labelledby="luna-apps-themes-disabled">
          <h2 id="luna-apps-themes-disabled">{t("apps.themes.disabled.title")}</h2>
          <p className="luna-apps__note">{t("apps.themes.disabled.lede")}</p>
          <ul className="luna-apps__list">
            {library.disabled.map((entry) => (
              <li key={entry.id} className="luna-apps__row">
                <span className="luna-apps__card-text">
                  <span className="luna-apps__card-name">{entry.name}</span>
                  <span className="luna-apps__meta">{t("apps.themes.disabled.reason", { reason: entry.reason })}</span>
                </span>
                <span className="luna-apps__row-side">
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary"
                    aria-label={t("apps.rowAction", { action: t("apps.themes.uninstall.action"), name: entry.name })}
                    onClick={() => void actions.uninstall(entry.id, entry.name, false)}
                  >
                    {t("apps.themes.uninstall.action")}
                  </button>
                </span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      {market ? (
        <section className="luna-apps__section" aria-labelledby="luna-apps-themes-market">
          {/* 「检查更新」只在「应用」页头一处（ADR-170 ③）；拉到新的一份时 Rust 广播 `themes://changed`，这一块自己重读。 */}
          <h2 id="luna-apps-themes-market">{t("apps.themes.market.title")}</h2>
          <p className="luna-apps__note">{t("apps.themes.market.lede")}</p>
          <ul className="luna-apps__list">
            {market.themes.map((theme) => (
              <MarketThemeRow
                key={theme.id}
                theme={theme}
                previewing={previewing?.id === theme.id}
                getting={getting === theme.id}
                disabled={getting !== null}
                onTryOn={() => tryOnMarket(theme)}
                onGet={() => void getTheme(theme)}
              />
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
}

/** 在线目录里的一行：按 Rust 给的状态摆按钮，这里不比版本。 */
function MarketThemeRow({
  theme,
  previewing,
  getting,
  disabled,
  onTryOn,
  onGet,
}: {
  theme: MarketThemeDto;
  previewing: boolean;
  getting: boolean;
  disabled: boolean;
  onTryOn: () => void;
  onGet: () => void;
}) {
  const { t } = useTranslation("workspace");
  const meta =
    theme.state === "update_available"
      ? t("apps.themes.market.updateMeta", {
          author: theme.author,
          license: theme.license,
          from: theme.installed_version ?? "",
          to: theme.version,
        })
      : t("apps.themes.market.meta", { author: theme.author, license: theme.license, version: theme.version });
  const action = theme.state === "update_available" ? t("apps.themes.market.update") : t("apps.themes.market.get");
  return (
    <li className="luna-apps__row">
      <span className="luna-apps__row-lead">
        <ThemeSwatch id={theme.id} />
        <span className="luna-apps__card-text">
          <span className="luna-apps__card-name">{theme.name}</span>
          <span className="luna-apps__meta">{meta}</span>
        </span>
      </span>
      <span className="luna-apps__row-side">
        {theme.state === "installed" ? (
          <TaskStatus kind="completed" label={t("apps.themes.market.installed")} />
        ) : theme.state === "needs_newer_app" ? (
          <span className="luna-apps__meta">{t("apps.themes.market.needsNewerApp")}</span>
        ) : (
          <>
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              aria-label={t("apps.rowAction", { action: t("apps.themes.tryOn"), name: theme.name })}
              disabled={previewing}
              onClick={onTryOn}
            >
              {t("apps.themes.tryOn")}
            </button>
            <button
              type="button"
              className="luna-btn luna-btn--primary"
              aria-label={t("apps.rowAction", { action, name: theme.name })}
              disabled={disabled}
              onClick={onGet}
            >
              {getting ? t("apps.themes.installing") : action}
            </button>
          </>
        )}
      </span>
    </li>
  );
}
