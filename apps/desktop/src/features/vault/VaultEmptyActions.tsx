/**
 * 没有打开的库时，空状态里的三条出路（ADR-151 修订 2026-10-15「续二」）：新建库、把已有文件夹
 * 打开为库、把当前账户的一个云端库下载到本机。
 *
 * 换账户之后新账户在这台设备上一个库都没打开过、而原来开着的库它看不见，Rust 就交出当前库停在
 * 这里——此刻用户要的是「我的库在哪」，不是去设置页。三条出路与「管理库」、切换菜单走同一份流程
 * （`vault-actions`），云端库来自同一份库清单订阅（`vault-list-store`），能不能下载由 Rust 判。
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { CloudVaultDto } from "../../ipc/bindings";
import { DownloadIcon, FolderIcon, PlusIcon } from "@lunarca/editor-core/Icon";
import { sameStructure } from "../../hooks/use-store-selector";
import { vaultErrorText } from "../../lib/vault-error";
import { syncErrorText } from "../account/sync-error";
import { pickParentTitle } from "../account/cloud-vault-download";
import { downloadCloudVaultHere, pickAndOpenVault } from "./vault-actions";
import { useVaultList } from "./vault-list-store";
import { useVaultPickPrompts } from "./use-vault-pick-prompts";

interface VaultEmptyActionsProps {
  /** 新建 / 打开文件夹成功换库后的收尾（宿主以此重挂载工作区）。 */
  onSwitched: () => void;
  /** 下载好的云端库：走切换器同一条护栏路径切过去。 */
  onOpenVault: (path: string, name: string) => void;
}

export function VaultEmptyActions({ onSwitched, onOpenVault }: VaultEmptyActionsProps) {
  const { t } = useTranslation("workspace");
  const { t: tCloud } = useTranslation("cloud");
  const cloud = useVaultList((snapshot) => snapshot.cloud, sameStructure);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const pickPrompts = useVaultPickPrompts();

  const open = async (create: boolean) => {
    const outcome = await pickAndOpenVault(create, pickPrompts);
    if (outcome.kind === "failed") {
      setError(vaultErrorText(outcome.code));
    } else if (outcome.kind === "done") {
      onSwitched();
    }
  };

  const download = async (vault: CloudVaultDto) => {
    setError(null);
    const outcome = await downloadCloudVaultHere(vault.id, pickParentTitle(vault.name, tCloud), () => setBusy(true));
    setBusy(false);
    if (outcome.kind === "failed") {
      setError(syncErrorText(outcome.code, tCloud));
    } else if (outcome.kind === "done") {
      onOpenVault(outcome.path, vault.name ?? outcome.path);
    }
  };

  return (
    <div className="luna-vault-empty">
      <div className="luna-vault-empty__actions">
        <button
          type="button"
          className="luna-btn luna-btn--primary"
          disabled={busy}
          onClick={() => void open(true)}
        >
          <PlusIcon size={14} />
          <span>{t("vault.create")}</span>
        </button>
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          disabled={busy}
          onClick={() => void open(false)}
        >
          <FolderIcon size={14} />
          <span>{t("vault.openFolder")}</span>
        </button>
      </div>
      {cloud.length > 0 ? (
        <div className="luna-vault-empty__cloud">
          <p className="luna-vault-empty__heading">{t("vault.sectionCloud")}</p>
          <ul className="luna-vault-empty__list">
            {cloud.map((vault) => (
              <li key={vault.id}>
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary"
                  disabled={busy}
                  title={t("vault.cloudDownloadHint")}
                  onClick={() => void download(vault)}
                >
                  <DownloadIcon size={14} />
                  <span>{vault.name ?? tCloud("sync.cloud_vaults.name_unreadable")}</span>
                </button>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      {busy ? (
        <p className="luna-dialog__note" role="status">
          {tCloud("sync.cloud_vaults.downloading")}
        </p>
      ) : null}
      {error !== null ? (
        <p className="luna-field-error" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}
