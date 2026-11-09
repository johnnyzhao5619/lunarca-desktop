/**
 * 管理库弹窗（PRD-07-064）：库清单的增删改与打开，一处收口。
 *
 * 与切库确认卡分工：**这里管清单**（有哪些库、叫什么名、还在不在），**切换的护栏与
 * 代价**归 `useVaultSwitch`。移除一条记录时必须把「不会删除磁盘文件」写在界面上——
 * 用户在一个叫「管理库」的地方点「移除」，不说清楚就有理由担心文件没了。
 *
 * 移除一个有云同步的库（ADR-151 修订 2026-10-15）：先问 Rust 它在这台设备上连着什么
 * （`forgetVaultPreflight`），据此确认「移除后这台设备将停止同步它；云端副本保留」；移除本身由 Rust
 * 顺带断开（同步列表只列库清单里的库）。没有同步的库照旧一点即移除。
 *
 * 库列表跟着账户走（同上修订「续二」）：这里列的是列表账户看得见的库（本机库 + 这个账户的库），
 * 与左下角切换菜单共用一份订阅（`vault-list-store`）——移除、改名、新建之后两处同时更新。移除
 * 本机库是从这台设备的清单删掉；移除账户的库只从这个账户的列表拿掉，别的账户不受影响（Rust 判）。
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { boundAccountParams } from "../../lib/bound-account";
import { useConfirm } from "../../components/ConfirmHost";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { commands, type VaultEntryDto, type VaultSyncLinkDto } from "../../ipc/bindings";
import { sameStructure } from "../../hooks/use-store-selector";
import { pickAndOpenVault } from "./vault-actions";
import { useVaultList } from "./vault-list-store";
import { useVaultPickPrompts } from "./use-vault-pick-prompts";
import { vaultErrorText } from "../../lib/vault-error";
import { syncErrorText } from "../account/sync-error";
import { isImeKeyEvent } from "../../lib/ime-key-event";

/**
 * 移除前的确认文案：只在它连着云同步时才问（`null` = 不问）。穷尽 `switch`——Rust 新增一档时
 * tsc 当场红。
 */
function forgetSyncCopy(
  link: VaultSyncLinkDto,
  t: (key: string, options?: Record<string, unknown>) => string,
): string | null {
  switch (link.kind) {
    case "none":
      return null;
    case "syncing":
      return t("vault.forgetSyncing");
    case "linked":
      return t("vault.forgetLinked", boundAccountParams(link.account, t));
  }
}

interface VaultManagerDialogProps {
  open: boolean;
  onClose: () => void;
  /** 打开某个库（走与切换器同一条护栏路径）。 */
  onOpenVault: (path: string, name: string) => void;
  /** 「新建库 / 打开文件夹为库」成功换库后的收尾（宿主以此重挂载工作区）。 */
  onSwitched?: () => void;
}

export function VaultManagerDialog({
  open,
  onClose,
  onOpenVault,
  onSwitched,
}: VaultManagerDialogProps) {
  const { t } = useTranslation("workspace");
  const { t: tCloud } = useTranslation("cloud");
  const ask = useConfirm();
  const pickPrompts = useVaultPickPrompts();
  const entries = useVaultList((snapshot) => snapshot.list?.vaults ?? [], sameStructure);
  const [renaming, setRenaming] = useState<{ path: string; value: string } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      setError(null);
    }
  }, [open]);

  // 模态行为（Esc / 焦点圈定 / 焦点归还）统一走共享 hook：此处此前只有一份手写的
  // Esc 监听，没有焦点圈定也没有焦点归还（十二张卡各写各的，能力参差）。
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose, enabled: open });

  const forget = useCallback(
    async (entry: VaultEntryDto) => {
      const link = await commands.forgetVaultPreflight(entry.path);
      const description = link.status === "ok" ? forgetSyncCopy(link.data, t) : null;
      if (description !== null) {
        const confirmed = await ask({
          title: t("vault.forgetSyncTitle", { name: entry.name }),
          description,
          confirmLabel: t("vault.forgetConfirm"),
        });
        if (!confirmed) return;
      }
      // 清单由 Rust 改写后广播（`vaults://changed`），这里与切换菜单同时更新，不自己重取。
      const result = await commands.forgetVault(entry.path);
      if (result.status === "error") {
        setError(syncErrorText(result.error, tCloud));
      } else {
        setError(null);
      }
    },
    [ask, t, tCloud],
  );

  /** 新建库 / 打开文件夹为库：与首启引导共用同一条创建与安全接入路径（DRY）。 */
  const addVault = useCallback(
    async (create: boolean) => {
      const outcome = await pickAndOpenVault(create, pickPrompts);
      if (outcome.kind === "cancelled") {
        return;
      }
      if (outcome.kind === "failed") {
        setError(vaultErrorText(outcome.code));
        return;
      }
      setError(null);
      onClose();
      onSwitched?.();
    },
    [onClose, onSwitched, pickPrompts],
  );

  if (!open) {
    return null;
  }

  return (
    <div className="luna-overlay" role="presentation" onClick={onClose}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-vault-manager"
        role="dialog"
        aria-modal="true"
        aria-label={t("vault.manageTitle")}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-dialog__header">
          <h3>{t("vault.manageTitle")}</h3>
          <p>{t("vault.manageDesc")}</p>
        </header>

        <ul className="luna-vault-manager__list">
          {entries.map((entry) => (
            <li
              key={entry.path}
              className="luna-vault-manager__row"
              data-missing={entry.missing || undefined}
            >
              <div className="luna-vault-manager__info">
                {renaming?.path === entry.path ? (
                  <input
                    className="luna-input"
                    value={renaming.value}
                    autoFocus
                    aria-label={t("vault.rename")}
                    onChange={(event) =>
                      setRenaming({ path: entry.path, value: event.currentTarget.value })
                    }
                    onBlur={() => setRenaming(null)}
                    onKeyDown={(event) => {
                      if (isImeKeyEvent(event)) return;
                      if (event.key === "Enter") {
                        const value = renaming.value.trim();
                        if (value) {
                          void commands.renameVault(entry.path, value).then(() => {
                            setRenaming(null);
                          });
                        }
                      } else if (event.key === "Escape") {
                        setRenaming(null);
                      }
                    }}
                  />
                ) : (
                  <button
                    type="button"
                    className="luna-vault-manager__name"
                    title={t("vault.rename")}
                    onClick={() => setRenaming({ path: entry.path, value: entry.name })}
                  >
                    {entry.name}
                    {entry.active && (
                      <span className="luna-vault-manager__badge">{t("vault.current")}</span>
                    )}
                    {entry.missing && (
                      <span className="luna-vault-manager__missing">{t("vault.missing")}</span>
                    )}
                  </button>
                )}
                <span className="luna-vault-manager__path" title={entry.path}>
                  {entry.path}
                </span>
              </div>
              <div className="luna-vault-manager__actions">
                {!entry.active && !entry.missing && (
                  <>
                    <button
                      type="button"
                      className="luna-btn luna-btn--secondary"
                      onClick={() => {
                        onClose();
                        onOpenVault(entry.path, entry.name);
                      }}
                    >
                      {t("vault.open")}
                    </button>
                    {/* 副库独立窗口（PRD-06-050）：受限浏览/编辑，主窗口不动。 */}
                    <button
                      type="button"
                      className="luna-btn luna-btn--secondary"
                      onClick={() => {
                        void commands.openVaultWindow(entry.path).then((result) => {
                          if (result.status === "error") {
                            setError(vaultErrorText(result.error));
                          } else {
                            setError(null);
                            onClose();
                          }
                        });
                      }}
                    >
                      {t("vault.openWindow")}
                    </button>
                  </>
                )}
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary"
                  onClick={() => void forget(entry)}
                >
                  {t("vault.forget")}
                </button>
              </div>
            </li>
          ))}
        </ul>

        {/* 「移除」的语义必须写在界面上——不写清楚，用户有理由担心文件被删了。 */}
        <p className="luna-dialog__note">{t("vault.forgetHint")}</p>
        {error !== null && (
          <p className="luna-field-error" role="alert">
            {error}
          </p>
        )}

        <div className="luna-dialog__actions">
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            onClick={() => void addVault(false)}
          >
            {t("vault.openFolder")}
          </button>
          <button
            type="button"
            className="luna-btn luna-btn--primary"
            onClick={() => void addVault(true)}
          >
            {t("vault.create")}
          </button>
        </div>
      </div>
    </div>
  );
}
