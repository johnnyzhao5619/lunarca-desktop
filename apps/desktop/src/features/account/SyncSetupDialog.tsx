/**
 * 云同步的开启 / 加入向导（ADR-148、ADR-150；实施方案 §7「开启向导」）。
 *
 * 同步跟着账户走（ADR-150）：没有同步密码、没有恢复密钥——库主密钥由账户托管，登录同一账户的
 * 设备自动取得。向导因此只有一步：看清说明，选「新建云端库」或「加入已有云端库」。
 *
 * 前端零业务（ADR-003）：建库、托管、取钥、绑定全在 Rust；这里只发命令、按稳定码出文案。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { commands, type MergePreviewDto, type RemoteVaultDto } from "../../ipc/bindings";
import { formatBytes } from "@lunarca/editor-core/format-bytes";
import { formatInstant } from "../../lib/format-datetime";
import { downloadCloudVault, pickParentTitle } from "./cloud-vault-download";
import { syncErrorText } from "./sync-error";

/** 「新建」、合并进某个云端库，或把它下载成新库（ADR-178 ④，只在登录后那一屏出现）。 */
type Choice = { kind: "new" } | { kind: "join"; id: string } | { kind: "download"; id: string };

export function SyncSetupDialog({
  vaultPath,
  vaultName,
  takeOver,
  localBytes,
  remainingBytes,
  linkedRemotes,
  firstLogin,
  onClose,
}: {
  /** 要开启同步的那个本地库（库注册表的键）。 */
  vaultPath: string;
  vaultName: string;
  /**
   * 这个库连在另一个账户下、用户已确认改为同步到当前账户（ADR-151 修订 2026-10-15）：原账户的
   * 绑定由 Rust 在新绑定落下时解除；向导取消则原样保留。
   */
  takeOver: boolean;
  /** 这个库同步范围的本地大小（开同步大约要占多少）；`null` = 还没算出来。 */
  localBytes: number | null;
  /** 账户容量还剩多少；`null` = 不限或还没取到。 */
  remainingBytes: number | null;
  /** 这台设备上已经有本地库对应的云端库：一个云端库只对应一个本地库，不再列出来。 */
  linkedRemotes: readonly string[];
  /** 登录后的那一屏（ADR-178 ②）：预选、可以「暂不同步」、放不下时录音默认不上传。 */
  firstLogin?: { recordingBytes: number };
  onClose: () => void;
}) {
  const { t, i18n } = useTranslation("cloud");
  const { t: tCommon } = useTranslation("common");
  const headingRef = useRef<HTMLHeadingElement>(null);
  const [vaults, setVaults] = useState<RemoteVaultDto[] | null>(null);
  const [choice, setChoice] = useState<Choice | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const submitting = useRef(false);
  const listSequence = useRef(0);
  const [error, setError] = useState<string | null>(null);
  const [preview, setPreview] = useState<MergePreviewDto | null>(null);
  const mounted = useRef(true);
  // 打开那一刻的快照：总览每来一次事件都是一个新数组，拿它当依赖会把清单重取一遍、
  // 把用户刚选的那一项冲掉。
  const linkedAtOpen = useRef(linkedRemotes);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // 在途时（建库、取钥）不许 Esc 关掉：关了也撤销不了已经发出的那一步。
  const dialogRef = useDialogBehavior<HTMLDivElement>({
    onClose,
    closeOnEscape: !busy,
    initialFocusRef: headingRef,
  });

  const loadVaults = useCallback(() => {
    const sequence = ++listSequence.current;
    setVaults(null);
    setListError(null);
    void commands.syncRemoteVaults().then((result) => {
      if (!mounted.current || sequence !== listSequence.current) return;
      if (result.status === "error") {
        setListError(result.error);
        return;
      }
      const list = result.data.filter(
        (vault) => !linkedAtOpen.current.includes(vault.id),
      );
      setVaults(list);
      if (firstLogin) {
        const newest = [...list].sort((a, b) => b.created_at.localeCompare(a.created_at))[0];
        setChoice(newest ? { kind: "join", id: newest.id } : { kind: "new" });
      }
    }).catch((cause: unknown) => {
      if (mounted.current && sequence === listSequence.current) setListError(String(cause));
    });
    // firstLogin 只决定打开那一刻的预选，不因父组件重渲染重取清单。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(loadVaults, [loadVaults]);

  const previewId = firstLogin && choice?.kind === "join" ? choice.id : null;
  useEffect(() => {
    setPreview(null);
    if (previewId === null) return;
    let live = true;
    void commands.syncMergePreview(vaultPath, previewId).then((result) => {
      if (live && result.status === "ok") setPreview(result.data);
    });
    return () => {
      live = false;
    };
  }, [previewId, vaultPath]);

  /** 放不下时录音默认不上传（ADR-178 ②）：登录后那一屏才替用户定，账户页的向导不动既有设置。 */
  const skipRecordings =
    firstLogin !== undefined &&
    firstLogin.recordingBytes > 0 &&
    remainingBytes !== null &&
    localBytes !== null &&
    localBytes > remainingBytes;

  const submit = async () => {
    if (!choice || submitting.current || vaults === null || listError) return;
    submitting.current = true;
    setBusy(true);
    setError(null);
    try {
    if (choice.kind === "download") {
      const vault = vaults.find((item) => item.id === choice.id);
      const outcome = await downloadCloudVault(choice.id, pickParentTitle(vault?.name ?? null, t));
      if (!mounted.current || outcome.kind === "cancelled") return;
      if (outcome.kind === "failed") {
        setError(outcome.code);
        return;
      }
      onClose();
      return;
    }
    if (skipRecordings) await commands.setSetting("sync.recordings_mode", { kind: "str", value: "off" });
    const result =
      choice.kind === "new"
        ? await commands.syncEnable(vaultPath, takeOver)
        : await commands.syncJoin(vaultPath, choice.id, takeOver);
    if (!mounted.current) return;
    if (result.status === "error") {
      setError(result.error);
      return;
    }
    onClose();
    } catch (cause) {
      if (mounted.current) setError(String(cause));
    } finally {
      submitting.current = false;
      if (mounted.current) setBusy(false);
    }
  };

  /** 库名解不开（端到端的高级档）时退回按创建时间认。 */
  const vaultLabel = (vault: RemoteVaultDto) => {
    const time = formatInstant(vault.created_at, i18n.language);
    const size = formatBytes(vault.bytes_stored, i18n.language);
    return vault.name
      ? t("sync.wizard.vault_option_named", { name: vault.name, time, size })
      : t("sync.wizard.vault_option", { time, size });
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
        role="dialog"
        aria-modal="true"
        aria-labelledby="cloud-sync-wizard-title"
        aria-busy={busy}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-dialog__header">
          <h3 id="cloud-sync-wizard-title" ref={headingRef} tabIndex={-1}>
            {t("sync.wizard.title", { name: vaultName })}
          </h3>
        </header>

        <form
          id="cloud-sync-setup"
          className="cloud-sync__body"
          onSubmit={(event) => {
            event.preventDefault();
            if (!busy) void submit();
          }}
        >
          <ul className="cloud-sync__points">
            <li>{t("sync.wizard.point_scope")}</li>
            <li>{t("sync.wizard.point_account")}</li>
            <li>{t("sync.wizard.point_encrypted")}</li>
            <li>{t("sync.wizard.point_metadata")}</li>
          </ul>
          <fieldset className="cloud-sync__choices" disabled={busy}>
            <legend className="cloud-sync__label">{t("sync.wizard.choose")}</legend>
            {(vaults ?? []).map((vault) => (
              <label key={vault.id} className="cloud-sync__choice">
                <input
                  type="radio"
                  name="cloud-sync-choice"
                  checked={choice?.kind === "join" && choice.id === vault.id}
                  onChange={() => setChoice({ kind: "join", id: vault.id })}
                />
                <span>{t("sync.wizard.choice_join", { vault: vaultLabel(vault) })}</span>
              </label>
            ))}
            {firstLogin
              ? (vaults ?? []).map((vault) => (
                  <label key={`download-${vault.id}`} className="cloud-sync__choice">
                    <input
                      type="radio"
                      name="cloud-sync-choice"
                      checked={choice?.kind === "download" && choice.id === vault.id}
                      onChange={() => setChoice({ kind: "download", id: vault.id })}
                    />
                    <span>{t("sync.wizard.choice_download", { vault: vaultLabel(vault) })}</span>
                  </label>
                ))
              : null}
            <label className="cloud-sync__choice">
              <input
                type="radio"
                name="cloud-sync-choice"
                checked={choice?.kind === "new"}
                onChange={() => setChoice({ kind: "new" })}
              />
              <span>{t("sync.wizard.choice_new")}</span>
            </label>
            {listError ? (
              <div>
                <p className="luna-field-error" role="alert">{syncErrorText(listError, t)}</p>
                <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm"
                  onClick={loadVaults}>{t("sync.wizard.retry")}</button>
              </div>
            ) : vaults === null ? (
              <p className="luna-settings-panel__hint" role="status">
                {t("sync.wizard.loading_vaults")}
              </p>
            ) : null}
          </fieldset>
          {choice ? <p className="luna-settings-panel__desc">
            {t(
              choice.kind === "new"
                ? "sync.wizard.new_desc"
                : choice.kind === "join"
                  ? "sync.wizard.join_desc"
                  : "sync.wizard.download_desc",
            )}
          </p> : null}
          {preview ? (
            <p className="cloud-sync__notice" data-tone={preview.differ > 0 ? "warning" : "muted"} role="status">
              {t("sync.wizard.preview", {
                download: preview.download,
                upload: preview.upload,
                size: formatBytes(preview.upload_bytes, i18n.language),
                same: preview.same,
                differ: preview.differ,
              })}
            </p>
          ) : null}
          {skipRecordings ? (
            <p className="cloud-sync__notice" data-tone="warning" role="status">
              {t("sync.wizard.recordings_stay")}
            </p>
          ) : null}
          {/* 开同步前就说清容量够不够（OneDrive「选择文件夹」同样先报所需空间）。只是提示、
              不拦：上传到上限后服务端会停在只读，删掉文件即可继续。 */}
          {choice?.kind === "new" && localBytes !== null ? (
            <p
              className="cloud-sync__notice"
              data-tone={
                remainingBytes !== null && localBytes > remainingBytes ? "warning" : "muted"
              }
              role="status"
            >
              {remainingBytes !== null && localBytes > remainingBytes
                ? t("sync.wizard.over_capacity", {
                    size: formatBytes(localBytes, i18n.language),
                    remaining: formatBytes(remainingBytes, i18n.language),
                  })
                : t("sync.wizard.upload_size", { size: formatBytes(localBytes, i18n.language) })}
            </p>
          ) : null}
        </form>

        {error ? (
          <p className="luna-field-error" role="alert">
            {syncErrorText(error, t)}
          </p>
        ) : null}
        {busy ? (
          <p className="luna-dialog__note" role="status">
            {t(choice?.kind === "new" ? "sync.wizard.enabling" : "sync.wizard.joining")}
          </p>
        ) : null}

        <div className="luna-dialog__actions">
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            disabled={busy}
            onClick={onClose}
          >
            {firstLogin ? t("sync.wizard.not_now") : tCommon("action.cancel")}
          </button>
          <button
            type="submit"
            form="cloud-sync-setup"
            className="luna-btn luna-btn--primary"
            disabled={busy || vaults === null || !choice || listError !== null}
          >
            {t(choice?.kind === "join" ? "sync.wizard.join" : "sync.wizard.create")}
          </button>
        </div>
      </div>
    </div>
  );
}
