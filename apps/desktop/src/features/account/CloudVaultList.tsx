/**
 * 账户页「云同步」一节里的「云端库」（ADR-151 修订 2026-10-15；TDD-14 §5）。
 *
 * 列出本账户的全部云端库——含不在这台设备上的与恢复期内已删除的：名字、云端占用、最近更新、在这台
 * 设备上的处境。每行能做什么（下载到本机 / 删除云端库 / 恢复）由 Rust 逐行裁决（`can_*`），这里只
 * 照着画按钮、发命令、按稳定码出文案（ADR-003）。设置中心仍是 13 个平铺页：这是同步一节里的一块。
 *
 * 删除云端库影响所有设备，故确认要照着输入库名（名字读不出时是 Rust 给的编号）；本地文件在任何
 * 设备上都不动，30 天内可在这里恢复。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";

import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { commands, type CloudVaultDto } from "../../ipc/bindings";
import { formatBytes } from "@lunarca/editor-core/format-bytes";
import { formatDate, formatDateTime } from "../../lib/format-datetime";
import { syncErrorText } from "./sync-error";
import { downloadCloudVault, pickParentTitle } from "./cloud-vault-download";

type Translate = (key: string, options?: Record<string, unknown>) => string;

/** 行里写的名字：读不出就如实说「名字暂不可读」，不编。 */
function displayName(vault: CloudVaultDto, t: Translate): string {
  return vault.name ?? t("sync.cloud_vaults.name_unreadable");
}

/** 删除确认：照着输入 Rust 给的那段字（库名，或名字读不出时的编号）才放行。 */
function DeleteCloudVaultDialog({
  vault,
  onClose,
  onDeleted,
}: {
  vault: CloudVaultDto;
  onClose: () => void;
  onDeleted: () => void;
}) {
  const { t } = useTranslation("cloud");
  const { t: tCommon } = useTranslation("common");
  const headingRef = useRef<HTMLHeadingElement>(null);
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dialogRef = useDialogBehavior<HTMLDivElement>({
    onClose,
    closeOnEscape: !busy,
    initialFocusRef: headingRef,
  });
  const matches = typed.trim() === vault.confirm_text;

  const submit = async () => {
    if (!matches || busy) return;
    setBusy(true);
    setError(null);
    const result = await commands.syncCloudVaultDelete(vault.id);
    setBusy(false);
    if (result.status === "error") {
      setError(result.error);
      return;
    }
    onDeleted();
  };

  return (
    <div
      className="luna-overlay"
      role="presentation"
      onClick={() => {
        if (!busy) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="luna-dialog cloud-sync__dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="cloud-vault-delete-title"
        aria-busy={busy}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-dialog__header">
          <h3 id="cloud-vault-delete-title" ref={headingRef} tabIndex={-1}>
            {vault.name
              ? t("sync.cloud_vaults.delete_dialog.title", { name: vault.name })
              : t("sync.cloud_vaults.delete_dialog.title_unreadable")}
          </h3>
          <p>{t("sync.cloud_vaults.delete_dialog.body")}</p>
        </header>
        <form
          id="cloud-vault-delete"
          className="cloud-sync__body"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <label className="cloud-sync__label" htmlFor="cloud-vault-delete-input">
            {t(
              vault.name
                ? "sync.cloud_vaults.delete_dialog.prompt"
                : "sync.cloud_vaults.delete_dialog.prompt_unreadable",
              { text: vault.confirm_text },
            )}
          </label>
          <input
            id="cloud-vault-delete-input"
            className="luna-input"
            value={typed}
            autoComplete="off"
            spellCheck={false}
            disabled={busy}
            aria-label={t("sync.cloud_vaults.delete_dialog.input_label")}
            onChange={(event) => setTyped(event.currentTarget.value)}
          />
        </form>
        {error ? (
          <p className="luna-field-error" role="alert">
            {syncErrorText(error, t)}
          </p>
        ) : null}
        {busy ? (
          <p className="luna-dialog__note" role="status">
            {t("sync.cloud_vaults.delete_dialog.deleting")}
          </p>
        ) : null}
        <div className="luna-dialog__actions">
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            disabled={busy}
            onClick={onClose}
          >
            {tCommon("action.cancel")}
          </button>
          <button
            type="submit"
            form="cloud-vault-delete"
            className="luna-btn luna-btn--danger"
            disabled={!matches || busy}
          >
            {t("sync.cloud_vaults.delete_dialog.confirm")}
          </button>
        </div>
      </div>
    </div>
  );
}

export function CloudVaultList({ refreshKey }: {
  /**
   * 这台设备同步着哪些云端库、各自上次同步到什么时候（总览拼成的键）：变了就重取，「已同步到…」
   * 跟着开关走，云端大小与更新时间跟着同步走。
   */
  refreshKey: string;
}) {
  const { t, i18n } = useTranslation("cloud");
  const [vaults, setVaults] = useState<CloudVaultDto[] | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<CloudVaultDto | null>(null);
  const sequence = useRef(0);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const load = useCallback(async () => {
    const current = ++sequence.current;
    setListError(null);
    const result = await commands.syncCloudVaults();
    if (!mounted.current || current !== sequence.current) return;
    if (result.status === "error") {
      setListError(result.error);
      return;
    }
    setVaults(result.data);
  }, []);

  useEffect(() => {
    void load();
  }, [load, refreshKey]);

  const download = useCallback(
    async (vault: CloudVaultDto) => {
      setActionError(null);
      setNotice(null);
      // 与库切换菜单「云端库」一节同一条流程（`cloud-vault-download`）。
      const outcome = await downloadCloudVault(vault.id, pickParentTitle(vault.name, t), () => setBusyId(vault.id));
      if (!mounted.current || outcome.kind === "cancelled") return;
      setBusyId(null);
      if (outcome.kind === "failed") {
        setActionError(outcome.code);
        return;
      }
      setNotice(t("sync.cloud_vaults.downloaded", { path: outcome.path }));
      void load();
    },
    [load, t],
  );

  const restore = useCallback(
    async (vault: CloudVaultDto) => {
      setActionError(null);
      setNotice(null);
      setBusyId(vault.id);
      const result = await commands.syncCloudVaultRestore(vault.id);
      if (!mounted.current) return;
      setBusyId(null);
      if (result.status === "error") {
        setActionError(result.error);
        void load();
        return;
      }
      setNotice(t("sync.cloud_vaults.restored"));
      void load();
    },
    [load, t],
  );

  const meta = (vault: CloudVaultDto) =>
    [
      t("sync.cloud_vaults.size", { size: formatBytes(vault.cloud_bytes, i18n.language) }),
      vault.last_changed_ms === null
        ? ""
        : t("sync.cloud_vaults.updated", {
            time: formatDateTime(vault.last_changed_ms, i18n.language),
          }),
    ]
      .filter(Boolean)
      .join(" · ");

  const active = (vaults ?? []).filter((vault) => vault.place.kind !== "deleted");
  const deleted = (vaults ?? []).filter((vault) => vault.place.kind === "deleted");

  const row = (vault: CloudVaultDto) => {
    const place = vault.place;
    const busy = busyId === vault.id;
    return (
      <li key={vault.id} className="luna-settings-panel__row cloud-sync__vault">
        <div className="luna-settings-panel__row-label">
          <span className="luna-settings-panel__row-title cloud-sync__vault-name">
            {displayName(vault, t)}
            {place.kind === "here" ? (
              <span className="cloud-sync__status" data-tone="success">
                {t("sync.cloud_vaults.here", { name: place.local_name })}
              </span>
            ) : place.kind === "not_here" ? (
              <span className="cloud-sync__status" data-tone="muted">
                {t("sync.cloud_vaults.not_here")}
              </span>
            ) : null}
          </span>
          <p className="cloud-sync__meta">{meta(vault)}</p>
          {place.kind === "here" ? (
            <p className="cloud-sync__path" title={place.vault_path}>
              {place.display_path}
            </p>
          ) : null}
          {place.kind === "deleted" ? (
            <p className="cloud-sync__meta">
              {vault.can_restore && place.purge_after_ms !== null
                ? t("sync.cloud_vaults.purge_on", {
                    date: formatDate(place.purge_after_ms, i18n.language),
                  })
                : t("sync.cloud_vaults.purge_due")}
            </p>
          ) : null}
          {vault.can_download || vault.can_delete || vault.can_restore ? (
            <div className="luna-settings-panel__actions">
              {vault.can_download ? (
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary luna-btn--sm"
                  disabled={busy}
                  onClick={() => void download(vault)}
                >
                  {busy ? t("sync.cloud_vaults.downloading") : t("sync.cloud_vaults.download")}
                </button>
              ) : null}
              {vault.can_restore ? (
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary luna-btn--sm"
                  disabled={busy}
                  onClick={() => void restore(vault)}
                >
                  {busy ? t("sync.cloud_vaults.restoring") : t("sync.cloud_vaults.restore")}
                </button>
              ) : null}
              {vault.can_delete ? (
                <button
                  type="button"
                  className="luna-btn luna-btn--danger luna-btn--sm"
                  disabled={busy}
                  onClick={() => {
                    setActionError(null);
                    setNotice(null);
                    setDeleting(vault);
                  }}
                >
                  {t("sync.cloud_vaults.delete")}
                </button>
              ) : null}
            </div>
          ) : null}
        </div>
      </li>
    );
  };

  return (
    <>
      <h3 className="luna-settings-panel__ledger-title">{t("sync.cloud_vaults.title")}</h3>
      <p className="luna-settings-panel__desc">{t("sync.cloud_vaults.desc")}</p>
      {listError ? (
        <div>
          <p className="luna-field-error" role="alert">
            {syncErrorText(listError, t)}
          </p>
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-btn--sm"
            onClick={() => void load()}
          >
            {t("sync.cloud_vaults.retry")}
          </button>
        </div>
      ) : vaults === null ? (
        <p className="luna-settings-panel__hint" role="status">
          {t("sync.cloud_vaults.loading")}
        </p>
      ) : null}
      {actionError ? (
        <p className="luna-field-error" role="alert">
          {syncErrorText(actionError, t)}
        </p>
      ) : null}
      {notice ? (
        <p className="cloud-sync__notice" role="status">
          {notice}
        </p>
      ) : null}
      {vaults !== null && active.length === 0 && deleted.length === 0 ? (
        <p className="luna-empty">{t("sync.cloud_vaults.empty")}</p>
      ) : null}
      {active.length > 0 ? (
        <ul className="luna-settings-panel__rows" aria-label={t("sync.cloud_vaults.title")}>
          {active.map(row)}
        </ul>
      ) : null}
      {deleted.length > 0 ? (
        <>
          <h4 className="luna-settings-panel__ledger-title">
            {t("sync.cloud_vaults.deleted_title")}
          </h4>
          <p className="cloud-sync__meta">{t("sync.cloud_vaults.deleted_desc")}</p>
          <ul
            className="luna-settings-panel__rows"
            aria-label={t("sync.cloud_vaults.deleted_title")}
          >
            {deleted.map(row)}
          </ul>
        </>
      ) : null}
      {/* 挂到 body：设置页的滚动容器不该成为遮罩的定位祖先（同确认宿主）。 */}
      {deleting
        ? createPortal(
            <DeleteCloudVaultDialog
              vault={deleting}
              onClose={() => setDeleting(null)}
              onDeleted={() => {
                setDeleting(null);
                void load();
              }}
            />,
            document.body,
          )
        : null}
    </>
  );
}
