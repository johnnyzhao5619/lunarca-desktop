/**
 * 主题库（ADR-169 ③ ④）：Rust 主题注册表的投影——内置主题与内容层里导入的主题包（可以多套），
 * 以及此刻在用的那一套。
 *
 * 设置 › 外观的主题面板与「应用 › 主题」页签共用这一份：有哪些主题、用哪套都由 Rust 判定
 * （`theme_library`：设置里的 id 对不上注册表时已回落 `luna`），这里不认识任何一个主题 id。
 * 随 `settings://changed`（`appearance.theme_family`）重读——导入、卸载、选用最后都会写它；主题目录下发封禁 /
 * 解除封禁时设置不变，随 `themes://changed` 重读。
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { commands, events, type DisabledThemeDto, type ThemeEntryDto } from "../ipc/bindings";

const THEME_FAMILY_KEY = "appearance.theme_family";
const SWATCH_STYLE_ID = "luna-theme-swatches";

/** 一张主题卡：注册表的条目 + 展示用的名字与一句说明。 */
export interface ThemeCard extends ThemeEntryDto {
  /** 内置主题取界面语言的译名，导入的用包里写的名字。 */
  displayName: string;
  /** 内置主题的一句话说明；导入的为 `null`（卡片改写作者与许可）。 */
  description: string | null;
}

export interface ThemeLibrary {
  /** 此刻在用的主题 id（`null` = 还没取回来）。 */
  inUse: string | null;
  /** 注册表里的全部主题，内置在前。 */
  themes: readonly ThemeCard[];
  /** 服务端因安全原因封禁、在本机停用的已装主题（不在 `themes` 里；解除封禁后自动回去）。 */
  disabled: readonly DisabledThemeDto[];
  /** 此刻能向在线主题目录「检查更新」（Rust 判：已登录、未开严格本地）；与「获取更多主题」那一块在不在无关。 */
  canCheckUpdates: boolean;
  /** 现在就重取一次（导入的结果要立刻反映在卡片上）。 */
  reload: () => void;
}

/** 装上卡片色块的变量（Rust 从校验过的配色生成）；重取时整段替换，不随面板卸载。 */
function applySwatchCss(themes: readonly ThemeEntryDto[]): void {
  let style = document.getElementById(SWATCH_STYLE_ID);
  if (!style) {
    style = document.createElement("style");
    style.id = SWATCH_STYLE_ID;
    document.head.append(style);
  }
  style.textContent = themes.map((theme) => theme.swatch_css).join("\n");
}

export function useThemeLibrary(): ThemeLibrary {
  const { t } = useTranslation("settings");
  const [inUse, setInUse] = useState<string | null>(null);
  const [entries, setEntries] = useState<readonly ThemeEntryDto[]>([]);
  const [disabled, setDisabled] = useState<readonly DisabledThemeDto[]>([]);
  const [canCheckUpdates, setCanCheckUpdates] = useState(false);

  const reload = useCallback(() => {
    void commands.themeLibrary().then((library) => {
      applySwatchCss(library.themes);
      setInUse(library.in_use);
      setEntries(library.themes);
      setDisabled(library.disabled);
      setCanCheckUpdates(library.can_check_updates);
    });
  }, []);

  useEffect(() => {
    reload();
    const unlisten = events.settingsChanged.listen((event) => {
      if (event.payload.key === THEME_FAMILY_KEY) reload();
    });
    // 主题目录下发了封禁 / 解除封禁：哪些可用、哪些已停用变了，设置却没变。
    const unlistenThemes = events.themesChanged.listen(() => reload());
    return () => {
      void unlisten.then((dispose) => dispose());
      void unlistenThemes.then((dispose) => dispose());
    };
  }, [reload]);

  const themes = entries.map((entry) =>
    entry.source === "builtin"
      ? {
          ...entry,
          displayName: t(`theme_library.builtin.${entry.id}.name`, { defaultValue: entry.name }),
          description: t(`theme_library.builtin.${entry.id}.desc`, { defaultValue: "" }) || null,
        }
      : { ...entry, displayName: entry.name, description: null },
  );
  return { inUse, themes, disabled, canCheckUpdates, reload };
}
