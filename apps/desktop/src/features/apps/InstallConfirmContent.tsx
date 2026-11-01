/**
 * 安装 / 修复确认框的正文（ADR-168 ③，对标 Chrome「添加扩展程序」）：「它可以：」逐条权限，
 * 要下载的东西合成一行写总大小与来源，本机已有的说一句「直接使用」。不点名组件——名字、版本与
 * 许可证在「开源组件与许可证」页（ADR-168 ⑤）。修复不重列权限（装的时候已经确认过）。
 */

import { useTranslation } from "react-i18next";

import type { AppPermissionDto } from "../../ipc/bindings";

export interface InstallConfirmContentProps {
  permissions: readonly AppPermissionDto[];
  /** 要下载的总大小（已格式化）；不用下载为 `null`。 */
  downloadSize: string | null;
  /** 有组件用的是本机已有的那一份。 */
  usesExisting: boolean;
}

export function InstallConfirmContent({ permissions, downloadSize, usesExisting }: InstallConfirmContentProps) {
  const { t } = useTranslation("workspace");
  return (
    <div className="luna-apps__confirm">
      {permissions.length > 0 ? (
        <>
          <p className="luna-apps__confirm-lead">{t("apps.install.canLead")}</p>
          <ul className="luna-apps__confirm-list" aria-label={t("apps.install.canLead")}>
            {permissions.map((kind) => (
              <li key={kind}>{t(`apps.can.${kind}`)}</li>
            ))}
          </ul>
        </>
      ) : null}
      {downloadSize !== null ? <p>{t("apps.install.downloads", { size: downloadSize })}</p> : null}
      {usesExisting ? <p>{t("apps.install.usesExisting")}</p> : null}
      <p className="luna-apps__note">{t("apps.install.note")}</p>
    </div>
  );
}
