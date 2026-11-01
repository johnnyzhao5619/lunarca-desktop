// 外观编排 hook（TDD-11 §主题 / a11y）。事实源在 Rust（appearance.* 设置）；此处只读取一次、
// 投影到根属性与原生外观，并在 system 模式下跟随系统明暗变化。无业务逻辑（ADR-003）。

import { useEffect } from "react";
import { commands, events, type SettingSpecDto, type SettingValueDto } from "../ipc/bindings";
import { i18n, resolveSupportedLanguage } from "../i18n/i18n";
import { SETTINGS_CHANGED_EVENT } from "../features/settings/use-settings";
import {
  applyBodyWidth,
  applyInstalledThemeCss,
  applyTheme,
  resolveDensity,
  type ThemeMode,
} from "./theme";

function strOf(value: SettingValueDto | undefined, fallback: string): string {
  return value?.kind === "str" ? value.value : fallback;
}

function boolOf(value: SettingValueDto | undefined, fallback: boolean): boolean {
  return value?.kind === "bool" ? value.value : fallback;
}

function intOf(value: SettingValueDto | undefined, fallback: number): number {
  return value?.kind === "int" ? value.value : fallback;
}

/**
 * @param initial 启动快照里的设置清单（`app_bootstrap`）：首次应用直接用它，不再单独读
 *   一遍全量设置（PRD-14-066 合并启动期读取）。`null` = 快照还没到，先不动；
 *   `standalone` = 不会有快照了（快照失败），自己读。
 */
export function useAppearance(initial: SettingSpecDto[] | null, standalone = false): void {
  useEffect(() => {
    if (initial === null && !standalone) {
      return;
    }
    let media: MediaQueryList | null = null;
    let onSystemChange: (() => void) | null = null;
    let disposed = false;
    let seeded = initial;

    function removeSystemListener() {
      if (media && onSystemChange) {
        media.removeEventListener("change", onSystemChange);
      }
      media = null;
      onSystemChange = null;
    }

    async function load() {
      removeSystemListener();
      // 首次用快照；之后任何一次「设置变了」都重读——快照只对启动那一刻成立。
      let specs = seeded;
      seeded = null;
      if (specs === null) {
        const result = await commands.listSettings();
        if (disposed || result.status !== "ok") {
          return;
        }
        specs = result.data;
      }
      const byKey = new Map(specs.map((spec) => [spec.key, spec.current_value]));
      const mode = strOf(byKey.get("appearance.theme"), "system") as ThemeMode;
      // 用哪套主题由 Rust 判定（设置里的 id 对不上已装的主题时已回落 `luna`，ADR-169 ④）。
      const theme = await commands.appearanceTheme();
      if (disposed) return;
      const family = theme.family;
      applyInstalledThemeCss(theme.css);
      const language = resolveSupportedLanguage(strOf(byKey.get("appearance.language"), "system"));
      const reduceMotion = boolOf(byKey.get("appearance.reduce_motion"), false);
      const highContrast = boolOf(byKey.get("appearance.high_contrast"), false);
      const fontScale = intOf(byKey.get("appearance.font_scale"), 100) / 100;
      const density = resolveDensity(strOf(byKey.get("appearance.density"), "compact"));

      const options = { mode, family, reduceMotion, highContrast, fontScale, density };
      applyTheme(options);
      applyBodyWidth(strOf(byKey.get("editor.body_width"), "medium"));
      document.documentElement.lang = language;
      void i18n.changeLanguage(language);
      void commands.setBackendLanguage(language);
      // 原生外观接收原始 mode：system 时由 AppKit 自行跟随系统（command 内 system → 清空 appearance）。
      void commands.setNativeTheme(mode);

      if (mode === "system") {
        media = window.matchMedia("(prefers-color-scheme: dark)");
        onSystemChange = () => {
          applyTheme(options);
        };
        media.addEventListener("change", onSystemChange);
      }
    }

    void load();
    const reload = () => {
      void load();
    };
    window.addEventListener(SETTINGS_CHANGED_EVENT, reload);
    const settingsChanged = events.settingsChanged.listen(() => reload());
    // 主题目录下发了封禁 / 解除封禁：设置没变，在用的主题却可能已不在注册表里（Rust 回落 Lunarca）。
    const themesChanged = events.themesChanged.listen(() => reload());
    return () => {
      disposed = true;
      window.removeEventListener(SETTINGS_CHANGED_EVENT, reload);
      void settingsChanged.then((dispose) => {
        dispose();
      });
      void themesChanged.then((dispose) => {
        dispose();
      });
      removeSystemListener();
    };
  }, [initial, standalone]);
}
