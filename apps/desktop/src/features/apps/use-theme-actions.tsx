/**
 * 主题的获取 / 更新 / 卸载与页上那一句结果（「应用 › 主题」与「应用 › 已安装」共用一份）。
 *
 * 判定都在 Rust：获取与更新按 id 从已验签目录重新取那一条（`install_market_theme`），卸载正在用的那套先切回
 * 内置（`uninstall_theme_package`）；这里只接按钮、弹确认框、按稳定错误码挑那句话。两处页签按同一组码说同一句话，
 * 不各写一份。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { commands, events } from "../../ipc/bindings";
import { useConfirm } from "../../components/ConfirmHost";
import type { ThemeLibrary } from "../../theme/use-theme-library";

/** 页上一句结果提示（文案键）。 */
export type ThemeNotice =
  | { key: "apps.themes.installed"; name: string }
  | { key: "apps.themes.market.updated"; name: string; version: string }
  | {
      key:
        | "apps.themes.rejected"
        | "apps.themes.alreadyInstalled"
        | "apps.themes.failed"
        | "apps.themes.saveFailed"
        | "apps.themes.market.unavailable"
        | "apps.themes.market.needsNewerApp";
    };

/** 安装失败的稳定错误码 → 页上那句话（从文件安装与在线获取走同一条管线、同一组码）。 */
export function importNotice(error: string): ThemeNotice {
  if (error.startsWith("THEME_IMPORT_REJECTED")) return { key: "apps.themes.rejected" };
  if (error.startsWith("THEME_ALREADY_INSTALLED")) return { key: "apps.themes.alreadyInstalled" };
  if (error.startsWith("THEME_MARKET_UNAVAILABLE") || error.startsWith("THEME_MARKET_NOT_FOUND")) {
    return { key: "apps.themes.market.unavailable" };
  }
  if (error.startsWith("THEME_MARKET_NEEDS_UPDATE")) return { key: "apps.themes.market.needsNewerApp" };
  return { key: "apps.themes.failed" };
}

/** 那一句话（成功是 `status`，失败是 `alert`）；没有结果时不渲染。 */
export function ThemeNoticeLine({ notice }: { notice: ThemeNotice | null }) {
  const { t } = useTranslation("workspace");
  if (notice === null) return null;
  const failed = notice.key !== "apps.themes.installed" && notice.key !== "apps.themes.market.updated";
  const text =
    notice.key === "apps.themes.installed"
      ? t(notice.key, { name: notice.name })
      : notice.key === "apps.themes.market.updated"
        ? t(notice.key, { name: notice.name, version: notice.version })
        : t(notice.key);
  return (
    <p className={failed ? "luna-apps__error" : "luna-apps__note"} role={failed ? "alert" : "status"}>
      {text}
    </p>
  );
}

export interface ThemeActions {
  notice: ThemeNotice | null;
  setNotice: (notice: ThemeNotice | null) => void;
  /** 正在获取 / 更新的那一套的 id。 */
  getting: string | null;
  /** 获取（第一次即启用）或更新（不改在用的那一套）一套在线主题。 */
  get: (id: string, update: boolean) => Promise<void>;
  /** 卸载一套导入或获取的主题；`inUse` 时确认框写明会先换回 Lunarca。已停用的不可能在用。 */
  uninstall: (id: string, name: string, inUse: boolean) => Promise<void>;
}

export interface ThemeActionHooks {
  /** 真要改动之前（卸载已确认、获取已开始）：如结束预览——预览不能压在刚换掉的主题上。 */
  beforeChange?: () => void;
  /** 获取或更新结束后（成败都调）要额外重读的东西，如「获取更多主题」那一块。 */
  onSettled?: () => void;
}

export function useThemeActions(library: ThemeLibrary, { beforeChange, onSettled }: ThemeActionHooks = {}): ThemeActions {
  const { t } = useTranslation("workspace");
  const confirm = useConfirm();
  const [notice, setNotice] = useState<ThemeNotice | null>(null);
  const [getting, setGetting] = useState<string | null>(null);

  // 主题目录换了一份（封禁、解除封禁、新版本）：上一次操作的那句话不再是此刻的事实（2026-10-22 真机：主题刚被封禁，
  // 「已安装」里还写着「已更新到 1.3.0」），清掉。
  useEffect(() => {
    const unlisten = events.themesChanged.listen(() => setNotice(null));
    return () => void unlisten.then((dispose) => dispose());
  }, []);

  const get = async (id: string, update: boolean) => {
    beforeChange?.();
    setGetting(id);
    setNotice(null);
    const result = await commands.installMarketTheme(id);
    setGetting(null);
    onSettled?.();
    if (result.status === "error") {
      setNotice(importNotice(result.error));
      return;
    }
    library.reload();
    setNotice(
      update
        ? { key: "apps.themes.market.updated", name: result.data.name, version: result.data.version }
        : { key: "apps.themes.installed", name: result.data.name },
    );
  };

  const uninstall = async (id: string, name: string, inUse: boolean) => {
    const ok = await confirm({
      title: t("apps.themes.uninstall.title", { name }),
      description: t(inUse ? "apps.themes.uninstall.descriptionInUse" : "apps.themes.uninstall.description"),
      confirmLabel: t("apps.themes.uninstall.confirm"),
      destructive: true,
    });
    if (!ok) return;
    beforeChange?.();
    setNotice(null);
    const result = await commands.uninstallThemePackage(id);
    if (result.status === "ok") library.reload();
    else setNotice({ key: "apps.themes.saveFailed" });
  };

  return { notice, setNotice, getting, get, uninstall };
}
