/**
 * 账户页「云同步」一节（ADR-148、ADR-150、ADR-151；实施方案 §7「账户页『同步』一节」）。
 *
 * 像 OneDrive「选择文件夹」那样逐库选择：列出这台设备的已知库，每行一个开关——打开 = 新建或
 * 加入云端库（向导），关闭 = 停止在这台设备上同步（本地文件与云端库都保留，其他设备照常）。
 * 之下是「云端库」（`CloudVaultList`）：账户里的全部云端库，含不在这台设备上的与已删除的。选中的库不论是否打开都实时同步。
 * 连在另一个账户下的库（ADR-151 修订 2026-10-15）开关为关，写明连在谁那儿；要由当前账户同步，
 * 先确认「改为同步到当前账户」再走同一个向导，新绑定落下时 Rust 才解除原账户的那一份。
 * 暂停两级：全部暂停（2 / 8 / 24 小时或直到恢复，到点自动恢复）与单库暂停，都跨重启保持。
 *
 * 只在已登录视图里出现，位于用量之后、设备之前。状态来自共享订阅（`useSyncOverview`），与状态栏
 * 同一份快照。前端零业务（ADR-003）：每个库在哪一步、计数、错误码都由 Rust 裁决。
 */

import { useCallback, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { boundAccountParams } from "../../lib/bound-account";

import { useConfirm } from "../../components/ConfirmHost";
import {
  syncAwaitsConfirmation,
  syncIsReadOnly,
  syncIsSelected,
  useSyncOverview,
} from "../../hooks/use-sync-overview";
import {
  commands,
  type SyncPhase,
  type SyncStorageDto,
  type UsageAlertDto,
  type VaultSyncDto,
} from "../../ipc/bindings";
import { formatBytes } from "@lunarca/editor-core/format-bytes";
import { formatDate, formatDateTime } from "../../lib/format-datetime";
import { CloudVaultList } from "./CloudVaultList";
import { syncErrorText } from "./sync-error";
import { SyncPreferences } from "./SyncPreferences";
import { SyncRestoreDialog } from "./SyncRestoreDialog";
import { SyncSetupDialog } from "./SyncSetupDialog";

const DAY_MS = 86_400_000;

/** 阶段 → 状态文案。穷尽 Record——Rust 新增一档时 tsc 当场红。 */
const PHASE_COPY: Record<SyncPhase, string> = {
  off: "sync.phase.off",
  locked: "sync.phase.locked",
  unavailable: "sync.phase.unavailable",
  idle: "sync.phase.idle",
  syncing: "sync.phase.syncing",
  paused: "sync.phase.paused",
  error: "sync.phase.error",
  other_account: "sync.phase.other_account",
};

/** 状态标签的色调（样式由 `sync.css` 按 `data-tone` 取状态色 Token）。 */
type Tone = "success" | "active" | "muted" | "warning" | "error";

const PHASE_TONE: Record<SyncPhase, Tone> = {
  off: "muted",
  locked: "warning",
  unavailable: "warning",
  idle: "success",
  syncing: "active",
  paused: "muted",
  error: "error",
  other_account: "muted",
};

/**
 * 一个库的状态标签：文案 + 色调。待确认删除与只读（超额 / 宽限）不是「出错稍后重试」，各自单说，
 * 与状态栏同一判据（`hooks/use-sync-overview`）。
 */
/** 太大没同步的文件：列前三个文件名（按界面语言的列举方式），其余由数量说明。 */
function tooLargeNames(paths: string[], language: string): string {
  const names = paths.slice(0, 3).map((path) => path.split("/").pop() ?? path);
  return new Intl.ListFormat(language, { type: "conjunction" }).format(names);
}

function statusOf(vault: VaultSyncDto): { label: string; tone: Tone } {
  if (syncIsSelected(vault) && syncAwaitsConfirmation(vault)) {
    return { label: "sync.status_awaiting", tone: "warning" };
  }
  if (syncIsReadOnly(vault)) return { label: "sync.read_only", tone: "warning" };
  return { label: PHASE_COPY[vault.phase], tone: PHASE_TONE[vault.phase] };
}

/** 全部暂停的时长选项（小时；`null` = 直到我恢复）。对标 OneDrive 2 / 8 / 24 小时与 Dropbox「无限期」。 */
const PAUSE_CHOICES: ReadonlyArray<{ hours: number | null; label: string }> = [
  { hours: 2, label: "sync.pause_all.hours_2" },
  { hours: 8, label: "sync.pause_all.hours_8" },
  { hours: 24, label: "sync.pause_all.hours_24" },
  { hours: null, label: "sync.pause_all.until_resumed" },
];

type Result = { status: string; error?: string };

/**
 * 容量徽标文案。档位是服务端用量里同步存储那一维的 `alert`（协议 v12，与用量表同一份判定）；
 * `none` 不出徽标。穷尽 Record——Rust 新增一档时 tsc 当场红。
 */
const STORAGE_ALERT_COPY: Record<Exclude<UsageAlertDto, "none">, string> = {
  p80: "sync.storage.alert_warning",
  p95: "sync.storage.alert_critical",
  exhausted: "sync.storage.alert_full",
};

/**
 * 账户容量卡片（ADR-152）：标题与「已用 / 上限」同一行、进度条占满整行、说明在下——与系统设置
 * 和 OneDrive 的存储条同形。「不限」不画进度条（画成满条会把随便用显示成快没了）；上限为 0 是
 * 「不含同步」，不画条。
 */
function StorageSummary({ storage }: { storage: SyncStorageDto }) {
  const { t, i18n } = useTranslation("cloud");
  const usedBytes = storage.used_bytes;
  const unlinkedBytes = storage.unlinked_bytes ?? 0;
  const used = formatBytes(usedBytes, i18n.language);
  const limitBytes = storage.limit.kind === "limited" ? storage.limit.bytes : null;
  const limited = limitBytes !== null && limitBytes > 0;
  // 满了是服务端说的（`exhausted`），不在这里拿已用与上限自己比。
  const full = storage.alert === "exhausted";
  return (
    <div className="cloud-sync__card">
      <div className="cloud-sync__card-head">
        <span className="cloud-sync__card-title">{t("sync.storage.title")}</span>
        <span className="cloud-sync__card-value">
          {limited
            ? t("sync.storage.amount", {
                used,
                limit: formatBytes(limitBytes, i18n.language),
              })
            : t(storage.limit.kind === "unlimited" ? "sync.storage.amount_unlimited"
                : storage.limit.kind === "not_included" ? "sync.storage.amount_not_included"
                : "sync.storage.amount_unknown", { used })}
          {storage.alert !== "none" ? (
            <span className="cloud-account__badge" data-alert={storage.alert}>
              {t(STORAGE_ALERT_COPY[storage.alert])}
            </span>
          ) : null}
        </span>
      </div>
      {limited && usedBytes !== null ? (
        <progress
          className="luna-progress cloud-sync__card-bar"
          value={usedBytes}
          max={limitBytes ?? 0}
        />
      ) : null}
      <p className="cloud-sync__meta">
        {unlinkedBytes > 0
          ? t("sync.storage.desc_unlinked", { size: formatBytes(unlinkedBytes, i18n.language) })
          : t("sync.storage.desc")}
      </p>
      {full ? (
        <p className="cloud-sync__notice" data-tone="warning" role="status">
          {t("sync.storage.full")}
        </p>
      ) : null}
    </div>
  );
}

export function SyncSection() {
  const { t, i18n } = useTranslation("cloud");
  const overview = useSyncOverview();
  const ask = useConfirm();
  /** 向导开给哪个库；`takeOver` = 连在另一个账户下、用户已确认改为同步到当前账户。 */
  const [setupFor, setSetupFor] = useState<{ vault: VaultSyncDto; takeOver: boolean } | null>(
    null,
  );
  /** 整库时间点恢复的那份库（PRD-13-007）。 */
  const [restoreFor, setRestoreFor] = useState<VaultSyncDto | null>(null);
  const [choosingPause, setChoosingPause] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  const act = useCallback(async (action: () => Promise<Result>) => {
    setActionError(null);
    const result = await action();
    if (result.status === "error" && result.error) setActionError(result.error);
  }, []);

  const stop = useCallback(
    async (vault: VaultSyncDto) => {
      const confirmed = await ask({
        title: t("sync.disable.title", { name: vault.name }),
        description: t("sync.disable.body"),
        confirmLabel: t("sync.disable.confirm"),
      });
      if (confirmed) void act(() => commands.syncDisable(vault.path));
    },
    [act, ask, t],
  );

  /** 改为同步到当前账户：先说清后果，确认了才打开向导（取消向导则什么都不变）。 */
  const takeOver = useCallback(
    async (vault: VaultSyncDto) => {
      const confirmed = await ask({
        title: t("sync.take_over.title", { name: vault.name }),
        description: t("sync.take_over.body"),
        confirmLabel: t("sync.take_over.confirm"),
      });
      if (confirmed) setSetupFor({ vault, takeOver: true });
    },
    [ask, t],
  );

  const confirmDeletions = useCallback(
    async (vault: VaultSyncDto) => {
      const confirmed = await ask({
        title: t("sync.confirm_delete.title", { name: vault.name }),
        description:
          vault.held_deletions > 0
            ? t("sync.confirm_delete.body_count", { count: vault.held_deletions })
            : t("sync.confirm_delete.body_empty"),
        confirmLabel: t("sync.confirm_delete.confirm"),
      });
      if (confirmed) void act(() => commands.syncConfirmBulkDelete(vault.path));
    },
    [act, ask, t],
  );

  if (!overview) return null;
  const ready = overview.gate === "ready";
  const anySelected = overview.vaults.some(syncIsSelected);
  const linkedRemotes = overview.vaults
    .map((v) => v.remote_vault_id)
    .filter((id): id is string => id !== null);
  const cloudListKey = overview.vaults
    .map((v) => `${v.remote_vault_id ?? ""}:${v.last_synced_ms ?? ""}`)
    .join(",");

  /** 大小与时间一行：选中的库「本地 · 云端 · 上次同步」，没选中的只有本地（开同步大约要占多少）。 */
  const metaLine = (vault: VaultSyncDto) => {
    const selected = syncIsSelected(vault);
    return [
      vault.local_bytes === null
        ? ""
        : t("sync.size.local", { local: formatBytes(vault.local_bytes, i18n.language) }),
      selected && vault.cloud_bytes !== null
        ? t("sync.size.cloud", { cloud: formatBytes(vault.cloud_bytes, i18n.language) })
        : "",
      selected && vault.last_synced_ms !== null
        ? t("sync.last_synced", { time: formatDateTime(vault.last_synced_ms, i18n.language) })
        : "",
    ]
      .filter(Boolean)
      .join(" · ");
  };

  const remainingBytes =
    overview.storage?.limit.kind === "limited" && overview.storage.limit.bytes !== null
      && overview.storage.used_bytes !== null
      ? Math.max(0, overview.storage.limit.bytes - overview.storage.used_bytes)
      : null;

  const purgeAfter = overview.read_only?.purge_after_ms ?? null;
  return (
    <section className="luna-settings-panel__section" aria-label={t("sync.title")}>
      <h2 className="luna-settings-panel__section-title">{t("sync.title")}</h2>
      <p className="luna-settings-panel__desc">{t("sync.choose_desc")}</p>
      {overview.gate === "strict_local" ? (
        <p className="cloud-sync__notice" data-tone="muted" role="status">
          {t("sync.strict_local")}
        </p>
      ) : null}
      {/* 只停了大文件（ADR-156 ③）：说清楚为什么、什么时候自动继续。 */}
      {overview.large_paused ? (
        <p className="cloud-sync__notice" data-tone="muted" role="status">
          {t(`sync.large_paused.${overview.large_paused}`)}
        </p>
      ) : null}
      {overview.paused ? (
        <p className="cloud-sync__notice" data-tone="warning" role="status">
          {overview.paused.until_ms === null
            ? t("sync.pause_all.paused_indefinitely")
            : t("sync.pause_all.paused_until", {
                time: formatDateTime(overview.paused.until_ms, i18n.language),
              })}
        </p>
      ) : null}

      {/* 失去同步权益（PRD-13-015）：只读，到期清云副本；本地从不受影响。 */}
      {ready && purgeAfter !== null ? (
        <p className="cloud-sync__notice" data-tone="warning" role="status">
          {t("sync.read_only_grace", {
            date: formatDate(purgeAfter, i18n.language),
            days: Math.max(0, Math.floor((purgeAfter - Date.now()) / DAY_MS)),
          })}
        </p>
      ) : null}

      {/* 账户被冻结（纵切 3 C3）：上传被服务端拒绝、下载照常；判据是服务端这一轮的答复。 */}
      {ready && overview.account_frozen ? (
        <p className="cloud-sync__notice" data-tone="error" role="status">
          {t("sync.account_frozen")}
        </p>
      ) : null}

      {ready ? overview.storage ? <StorageSummary storage={overview.storage} /> : (
        <p className="luna-settings-panel__hint" role="status">{t("sync.storage.unavailable")}</p>
      ) : null}

      {/* 全部同步的动作挨着容量放，而不是沉在一长串库的下面。 */}
      {ready && anySelected ? (
        choosingPause ? (
          <div
            className="luna-settings-panel__actions"
            role="group"
            aria-label={t("sync.pause_all.choose")}
          >
            {PAUSE_CHOICES.map((choice) => (
              <button
                key={choice.label}
                type="button"
                className="luna-btn luna-btn--secondary luna-btn--sm"
                onClick={() => {
                  setChoosingPause(false);
                  void act(() => commands.syncPauseAll(choice.hours));
                }}
              >
                {t(choice.label)}
              </button>
            ))}
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-btn--sm"
              onClick={() => setChoosingPause(false)}
            >
              {t("sync.pause_all.cancel")}
            </button>
          </div>
        ) : (
          <div className="luna-settings-panel__actions">
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-btn--sm"
              disabled={overview.paused !== null}
              onClick={() => void act(() => commands.syncNow())}
            >
              {t("sync.now")}
            </button>
            {overview.paused ? (
              <button
                type="button"
                className="luna-btn luna-btn--primary luna-btn--sm"
                onClick={() => void act(() => commands.syncResumeAll())}
              >
                {t("sync.pause_all.resume")}
              </button>
            ) : (
              <button
                type="button"
                className="luna-btn luna-btn--secondary luna-btn--sm"
                onClick={() => setChoosingPause(true)}
              >
                {t("sync.pause_all.open")}
              </button>
            )}
          </div>
        )
      ) : null}

      {actionError ? (
        <p className="luna-field-error" role="alert">
          {syncErrorText(actionError, t)}
        </p>
      ) : null}

      {overview.vaults.length === 0 ? (
        <p className="luna-empty">{t("sync.no_vaults")}</p>
      ) : (
        <ul className="luna-settings-panel__rows">
          {overview.vaults.map((vault) => {
            const selected = syncIsSelected(vault);
            const elsewhere = vault.phase === "other_account";
            const awaiting = selected && syncAwaitsConfirmation(vault);
            const status = statusOf(vault);
            const meta = metaLine(vault);
            return (
              <li key={vault.path} className="luna-settings-panel__row cloud-sync__vault">
                <div className="luna-settings-panel__row-label">
                  {/* 第一行：库名 + 当前打开 + 状态标签。 */}
                  <span className="luna-settings-panel__row-title cloud-sync__vault-name">
                    {vault.name}
                    {vault.is_open ? (
                      <span className="cloud-account__badge">{t("sync.open_badge")}</span>
                    ) : null}
                    <span className="cloud-sync__status" data-tone={status.tone}>
                      {vault.bound_account
                        ? t(status.label, boundAccountParams(vault.bound_account, t))
                        : t(status.label)}
                    </span>
                  </span>
                  {/* 第二行：大小与时间；第三行：路径（单独一行、超长省略，悬停看全路径）。 */}
                  {meta ? <p className="cloud-sync__meta">{meta}</p> : null}
                  <p className="cloud-sync__path" title={vault.path}>
                    {vault.display_path}
                  </p>
                  {elsewhere ? (
                    <p className="cloud-sync__notice" data-tone="muted" role="status">
                      {t("sync.other_account")}
                    </p>
                  ) : null}
                  {elsewhere && ready ? (
                    <div className="luna-settings-panel__actions">
                      <button
                        type="button"
                        className="luna-btn luna-btn--secondary luna-btn--sm"
                        onClick={() => void takeOver(vault)}
                      >
                        {t("sync.take_over.open")}
                      </button>
                    </div>
                  ) : null}
                  {selected && vault.phase === "error" && vault.last_error && !awaiting ? (
                    <p
                      className="cloud-sync__notice"
                      data-tone={syncIsReadOnly(vault) ? "warning" : "error"}
                      role="status"
                    >
                      {syncErrorText(vault.last_error, t)}
                    </p>
                  ) : null}
                  {awaiting ? (
                    <p className="cloud-sync__notice" data-tone="warning" role="status">
                      {vault.held_deletions > 0
                        ? t("sync.held_deletions", { count: vault.held_deletions })
                        : syncErrorText(vault.last_error ?? "", t)}
                    </p>
                  ) : null}
                  {selected && vault.conflict_copies > 0 ? (
                    <p className="cloud-sync__notice" data-tone="muted" role="status">
                      {t("sync.conflict_copies", { count: vault.conflict_copies })}
                    </p>
                  ) : null}
                  {selected && vault.too_large.length > 0 ? (
                    <p className="cloud-sync__notice" data-tone="warning" role="status">
                      {t("sync.too_large", {
                        count: vault.too_large.length,
                        names: tooLargeNames(vault.too_large, i18n.language),
                      })}
                    </p>
                  ) : null}
                  {selected && ready ? (
                    <div className="luna-settings-panel__actions">
                      {awaiting ? (
                        <button
                          type="button"
                          className="luna-btn luna-btn--danger luna-btn--sm"
                          onClick={() => void confirmDeletions(vault)}
                        >
                          {t("sync.confirm_delete.open")}
                        </button>
                      ) : null}
                      <button
                        type="button"
                        className="luna-btn luna-btn--secondary luna-btn--sm"
                        onClick={() =>
                          void act(() => commands.syncSetVaultPaused(vault.path, !vault.paused))
                        }
                      >
                        {t(vault.paused ? "sync.resume_vault" : "sync.pause_vault")}
                      </button>
                      <button
                        type="button"
                        className="luna-btn luna-btn--secondary luna-btn--sm"
                        onClick={() => setRestoreFor(vault)}
                      >
                        {t("sync.restore.open")}
                      </button>
                    </div>
                  ) : null}
                </div>
                <button
                  type="button"
                  role="switch"
                  aria-checked={selected}
                  aria-label={t("sync.vault_toggle", { name: vault.name })}
                  className="luna-switch"
                  disabled={!ready}
                  onClick={() => {
                    if (selected) void stop(vault);
                    else if (elsewhere) void takeOver(vault);
                    else setSetupFor({ vault, takeOver: false });
                  }}
                >
                  <span className="luna-switch__track" aria-hidden="true">
                    <span className="luna-switch__thumb" />
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}

      {/* 账户里的全部云端库（ADR-151 修订 2026-10-15）：下载到本机、删除与恢复。 */}
      {ready ? <CloudVaultList refreshKey={cloudListKey} /> : null}

      {/* 这台设备的同步偏好（录音同步方式、大文件暂停，ADR-156）：对所有选中的库生效。 */}
      {ready ? (
        <>
          <h3 className="luna-settings-panel__ledger-title">{t("sync.preferences_title")}</h3>
          <SyncPreferences />
        </>
      ) : null}

      {restoreFor
        ? createPortal(
            <SyncRestoreDialog
              vaultPath={restoreFor.path}
              vaultName={restoreFor.name}
              onClose={() => setRestoreFor(null)}
            />,
            document.body,
          )
        : null}
      {/* 挂到 body：设置页的滚动容器不该成为遮罩的定位祖先（同确认宿主）。 */}
      {setupFor
        ? createPortal(
            <SyncSetupDialog
              vaultPath={setupFor.vault.path}
              vaultName={setupFor.vault.name}
              takeOver={setupFor.takeOver}
              localBytes={setupFor.vault.local_bytes}
              remainingBytes={remainingBytes}
              linkedRemotes={linkedRemotes}
              onClose={() => setSetupFor(null)}
            />,
            document.body,
          )
        : null}
    </section>
  );
}
