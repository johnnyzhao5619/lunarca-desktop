import { useTranslation } from "react-i18next";
import type { SettingSpecDto } from "../../ipc/bindings";
import { ThemeSwatch } from "../../components/ThemeSwatch";
import { goTo } from "../../lib/navigate";
import { useThemeLibrary } from "../../theme/use-theme-library";
import type { SetValue } from "./SettingControls";

/**
 * 内置主题与导入的主题包（可以多套）的选择面：设置里只「选用哪一套」；预览、从文件安装与卸载在
 * 「应用 › 主题」（ADR-168 ⑦），这里给一条去那里的路。有哪些主题、在用哪一套都是 Rust 主题注册表
 * 的投影（`useThemeLibrary`，ADR-169 ③）。装上的主题在在线目录里有新版本时（Rust 给的 `update_to`），卡片上写
 * 「可更新到 …」（PRD-18-011：对应选择面也要标出）；更新本身只在「应用」里做，这里不放第二个入口。
 */
export function ThemeLibraryPanel({
  spec,
  setValue,
}: {
  spec: SettingSpecDto | undefined;
  setValue: SetValue;
}) {
  const { t } = useTranslation("settings");
  const library = useThemeLibrary();
  const selected = library.inUse;

  async function select(family: string) {
    if (!spec || family === selected) return;
    await setValue(spec.key, { kind: "str", value: family });
  }

  const entries = library.themes.map((theme) => ({
    id: theme.id,
    name: theme.displayName,
    desc:
      theme.description ??
      (theme.update_to !== null
        ? t("theme_library.update_meta", { author: theme.author, license: theme.license, version: theme.update_to })
        : t("theme_library.installed_meta", { author: theme.author, license: theme.license })),
  }));

  return (
    <section
      id="settings-section-theme_library"
      tabIndex={-1}
      className="luna-settings-panel luna-theme-library"
      aria-label={t("theme_library.title")}
    >
      <header className="luna-settings-panel__header">
        <div>
          <h2>{t("theme_library.title")}</h2>
          <p className="luna-settings-panel__hint">{t("theme_library.hint")}</p>
        </div>
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          onClick={() => goTo("apps://themes")}
        >
          {t("theme_library.browse")}
        </button>
      </header>
      <div className="luna-theme-library__grid" role="radiogroup" aria-label={t("theme_library.title")}>
        {entries.map((entry) => (
          <button
            key={entry.id}
            type="button"
            role="radio"
            aria-checked={selected === entry.id}
            className="luna-theme-library__item"
            onClick={() => void select(entry.id)}
          >
            <span className="luna-theme-library__mark" aria-hidden="true" />
            <ThemeSwatch id={entry.id} />
            <span className="luna-theme-library__copy">
              <strong>{entry.name}</strong>
              <small>{entry.desc}</small>
            </span>
            {selected === entry.id ? (
              <span className="luna-theme-library__selected">{t("theme_library.selected")}</span>
            ) : null}
          </button>
        ))}
      </div>
    </section>
  );
}
