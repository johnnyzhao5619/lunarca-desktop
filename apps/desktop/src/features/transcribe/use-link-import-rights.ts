/**
 * 「从链接导入」第一次使用前的权利声明（ADR-167 ④）：只导入你拥有或已获授权使用的内容、遵守
 * 来源平台的条款、Lunarca 不绕过登录 / 付费墙 / DRM。确认过一次就记在设备上，之后不再问。
 *
 * 记没记过由 Rust 给（`fetch_tool_status.rights_confirmed`），入队时 Rust 还会再查一遍兜底；
 * 这里只负责在入队之前问一次。转写页的链接导入与创作「添加来源 ▸ 视频链接」共用。
 */

import { useCallback } from "react";
import { useTranslation } from "react-i18next";

import { commands } from "../../ipc/bindings";
import { useConfirm } from "../../components/ConfirmHost";

/** 返回「确认过了没有」：没确认过就弹确认卡，用户点了确认才记下并返回 true。 */
export function useLinkImportRights(): () => Promise<boolean> {
  const { t } = useTranslation("transcribe");
  const confirm = useConfirm();
  return useCallback(async () => {
    const status = await commands.fetchToolStatus(false);
    if (status.status === "ok" && status.data.rights_confirmed) return true;
    const ok = await confirm({
      title: t("fetch.rights.title"),
      description: t("fetch.rights.description"),
      confirmLabel: t("fetch.rights.confirm"),
      destructive: false,
    });
    if (!ok) return false;
    const saved = await commands.confirmLinkImportRights();
    return saved.status === "ok";
  }, [confirm, t]);
}
