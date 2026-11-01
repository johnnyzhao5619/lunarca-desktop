/**
 * 「应用 › 已安装」里的主题一节（PRD-18-011 / 012 / 016，ADR-168 ⑦：「已安装」取代「我的内容」）：内容层里的
 * 主题——从文件导入的与从在线目录获取的走同一条管线、同一种行——连同因安全原因停用的。可更新的标出目标版本、
 * 给「更新」（用户点了才装）；都能卸载。内置主题不在这里（它们不是装上的，也卸不掉）。
 *
 * 「可更新到哪一版」由 Rust 算好（`ThemeEntryDto.update_to`，读已验签目录的本地副本），这里不比版本；获取、更新、
 * 卸载与那一句结果和「应用 › 主题」共用 `useThemeActions`。预览与使用在「主题」页签。
 */

import { useTranslation } from "react-i18next";

import { TaskStatus } from "../../components/TaskStatus";
import { ThemeSwatch } from "../../components/ThemeSwatch";
import type { ThemeLibrary } from "../../theme/use-theme-library";
import { ThemeNoticeLine, useThemeActions } from "./use-theme-actions";

/** 这一节有几行、其中几行可更新（页签上的数字与它同源）。 */
export function installedThemeCounts(library: ThemeLibrary): { count: number; updates: number } {
  const installed = library.themes.filter((entry) => entry.source === "content");
  return {
    count: installed.length + library.disabled.length,
    updates: installed.filter((entry) => entry.update_to !== null).length,
  };
}

export function InstalledThemes({ library }: { library: ThemeLibrary }) {
  const { t } = useTranslation("workspace");
  const actions = useThemeActions(library);
  const installed = library.themes.filter((entry) => entry.source === "content");
  if (installed.length === 0 && library.disabled.length === 0) return null;
  return (
    <section className="luna-apps__section" aria-labelledby="luna-apps-installed-themes">
      <h2 id="luna-apps-installed-themes">{t("apps.tab.themes")}</h2>
      <ThemeNoticeLine notice={actions.notice} />
      <ul className="luna-apps__list">
        {installed.map((entry) => (
          <li key={entry.id} className="luna-apps__row">
            <span className="luna-apps__row-lead">
              <ThemeSwatch id={entry.id} />
              <span className="luna-apps__card-text">
                <span className="luna-apps__card-name">{entry.displayName}</span>
                <span className="luna-apps__meta">
                  {entry.update_to !== null
                    ? t("apps.themes.market.updateMeta", {
                        author: entry.author,
                        license: entry.license,
                        from: entry.version,
                        to: entry.update_to,
                      })
                    : t("apps.themes.localMeta", { author: entry.author, license: entry.license, version: entry.version })}
                </span>
              </span>
            </span>
            <span className="luna-apps__row-side">
              {library.inUse === entry.id ? <TaskStatus kind="completed" label={t("apps.themes.inUse")} /> : null}
              {entry.update_to !== null ? (
                <button
                  type="button"
                  className="luna-btn luna-btn--primary"
                  aria-label={t("apps.rowAction", { action: t("apps.themes.market.update"), name: entry.displayName })}
                  disabled={actions.getting !== null}
                  onClick={() => void actions.get(entry.id, true)}
                >
                  {actions.getting === entry.id ? t("apps.themes.installing") : t("apps.themes.market.update")}
                </button>
              ) : null}
              <button
                type="button"
                className="luna-btn luna-btn--secondary"
                aria-label={t("apps.rowAction", { action: t("apps.themes.uninstall.action"), name: entry.displayName })}
                disabled={actions.getting === entry.id}
                onClick={() => void actions.uninstall(entry.id, entry.displayName, library.inUse === entry.id)}
              >
                {t("apps.themes.uninstall.action")}
              </button>
            </span>
          </li>
        ))}
        {library.disabled.map((entry) => (
          <li key={entry.id} className="luna-apps__row">
            <span className="luna-apps__card-text">
              <span className="luna-apps__card-name">{entry.name}</span>
              <span className="luna-apps__meta">{t("apps.themes.disabled.reason", { reason: entry.reason })}</span>
            </span>
            <span className="luna-apps__row-side">
              <TaskStatus kind="idle" label={t("apps.themes.disabled.title")} />
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
  );
}
