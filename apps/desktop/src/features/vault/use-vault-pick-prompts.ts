/**
 * 新建库 / 打开文件夹为库时要对用户说的两句话（「管理库」与无库空状态共用，DRY）。
 *
 * 判定全在 Rust（`default_vault_path` 的 `avoided_onedrive`、`vault_path_in_onedrive`，ADR-003）；
 * 这里只把文案与确认卡交给 `pickAndOpenVault`。
 */

import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { useConfirm } from "../../components/ConfirmHost";
import type { VaultPickPrompts } from "./vault-actions";

export function useVaultPickPrompts(): VaultPickPrompts {
  const { t } = useTranslation("workspace");
  const confirm = useConfirm();
  return useMemo(
    () => ({
      oneDriveTitle: (suggested: string) => t("vault.oneDrivePickTitle", { path: suggested }),
      confirmOneDrive: () =>
        confirm({
          title: t("vault.oneDriveConfirmTitle"),
          description: t("vault.oneDriveConfirmBody"),
          confirmLabel: t("vault.oneDriveConfirmUse"),
          destructive: false,
        }),
    }),
    [t, confirm],
  );
}
