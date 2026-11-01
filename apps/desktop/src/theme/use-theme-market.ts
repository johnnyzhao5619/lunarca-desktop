/**
 * 在线主题目录（纵切 7a，ADR-159 ③）：Rust `theme_market` 的投影——「获取更多主题」里有哪些、各是什么状态。
 *
 * 这里不判断任何事：入口该不该出现（严格本地、未登录、服务端没发布主题目录时 Rust 回 `null`）、每一条是
 * 可获取 / 已安装 / 可更新 / 需要更新 Lunarca，都由 Rust 算好。命令零网络（读已验签目录的本地副本），
 * 随 `settings://changed`（`appearance.theme_family`）重读——获取与更新最后都会发它；主题目录换了一份时随
 * `themes://changed` 重读。
 */

import { useCallback, useEffect, useState } from "react";

import { commands, events, type ThemeMarketDto } from "../ipc/bindings";

const THEME_FAMILY_KEY = "appearance.theme_family";
const SWATCH_STYLE_ID = "luna-theme-market-swatches";

/** 装上在线主题卡片色块的变量；与主题库那一段分开放，两边各自整段替换、互不覆盖。 */
function applySwatchCss(market: ThemeMarketDto | null): void {
  let style = document.getElementById(SWATCH_STYLE_ID);
  if (!style) {
    style = document.createElement("style");
    style.id = SWATCH_STYLE_ID;
    document.head.append(style);
  }
  style.textContent = market === null ? "" : market.themes.map((theme) => theme.swatch_css).join("\n");
}

export interface ThemeMarket {
  /** `undefined` = 还没取回来；`null` = 这一块不出现。 */
  market: ThemeMarketDto | null | undefined;
  reload: () => void;
}

export function useThemeMarket(): ThemeMarket {
  const [market, setMarket] = useState<ThemeMarketDto | null | undefined>(undefined);

  const reload = useCallback(() => {
    void commands.themeMarket().then((next) => {
      applySwatchCss(next);
      setMarket(next);
    });
  }, []);

  useEffect(() => {
    reload();
    const unlisten = events.settingsChanged.listen((event) => {
      if (event.payload.key === THEME_FAMILY_KEY) reload();
    });
    // 主题目录换了一份（新条目、新版本、封禁）：这一块要跟着变，不必等用户离开再回来。
    const unlistenThemes = events.themesChanged.listen(() => reload());
    return () => {
      void unlisten.then((dispose) => dispose());
      void unlistenThemes.then((dispose) => dispose());
    };
  }, [reload]);

  return { market, reload };
}
