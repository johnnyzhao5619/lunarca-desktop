/**
 * 文件树上「只在云端」的文件（官方同步按需下载，ADR-156）：下载到本机、释放本机空间与下载进度。
 *
 * 判定全在 Rust：哪一行只在云端（`sync_cloud_only`）、哪一行是录音（`media`）、能不能释放
 * （本机这份与云端一致才放），这里只发命令、订阅进度、把结果说成一句话。
 *
 * **整棵树只订它真正要的那两样**（2026-10-15 真机卡顿）：同步每一轮要发好几次总览，而树只
 * 关心「当前库开没开同步」这一位，故经选择器取布尔，翻转时才重渲染；下载进度只属于正在下载
 * 的那一行，故由行上的徽标经 `useDownloadPercent` 自己订，不经整棵树。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { useStoreSelector } from "../../hooks/use-store-selector";
import { syncIsSelected, useSyncOverviewSelector } from "../../hooks/use-sync-overview";
import { commands, events } from "../../ipc/bindings";

/** 正在下载的文件 → 已下的百分比（一份树一份；行上的徽标只订自己那一行）。 */
export interface DownloadProgress {
  subscribe: (onChange: () => void) => () => void;
  /** 此刻的全部进度（不可变快照：每次变化换一份新的）。 */
  snapshot: () => ReadonlyMap<string, number>;
  /** 这一行此刻的百分比；没在下载 = `undefined`。 */
  get: (path: string) => number | undefined;
}

export interface SyncCloudOnly {
  /** 当前打开的库开着同步（释放本机空间只在这时给）。 */
  synced: boolean;
  progress: DownloadProgress;
  /** 下载到本机；下好了（或本来就在）返回 `true`。 */
  download: (path: string, name: string) => Promise<boolean>;
  evict: (path: string, name: string) => Promise<void>;
}

function createDownloadProgress(): DownloadProgress & {
  set: (path: string, percent: number | null) => void;
} {
  let percents: ReadonlyMap<string, number> = new Map();
  const listeners = new Set<() => void>();
  return {
    subscribe: (onChange) => {
      listeners.add(onChange);
      return () => listeners.delete(onChange);
    },
    snapshot: () => percents,
    get: (path) => percents.get(path),
    set: (path, percent) => {
      if ((percents.get(path) ?? null) === percent) return;
      const next = new Map(percents);
      if (percent === null) {
        next.delete(path);
      } else {
        next.set(path, percent);
      }
      percents = next;
      for (const listener of listeners) listener();
    },
  };
}

/** 一行的下载百分比：只在**这一行**的数字变了时重渲染调用方。 */
export function useDownloadPercent(progress: DownloadProgress, path: string): number | undefined {
  return useStoreSelector(progress.subscribe, progress.snapshot, (percents) => percents.get(path));
}

export function useSyncCloudOnly(onStatus: (text: string) => void): SyncCloudOnly {
  const { t } = useTranslation("workspace");
  const synced = useSyncOverviewSelector(
    (overview) => overview?.vaults.some((vault) => vault.is_open && syncIsSelected(vault)) ?? false,
  );
  const [progress] = useState(createDownloadProgress);

  useEffect(() => {
    const unlisten = events.syncDownloadProgress.listen(({ payload }) => {
      const done = payload.done ?? 0;
      const total = payload.total ?? 0;
      progress.set(payload.path, total > 0 && done < total ? Math.floor((done * 100) / total) : null);
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [progress]);

  const settle = (path: string) => progress.set(path, null);

  const download = async (path: string, name: string) => {
    onStatus(t("cloud.sync_downloading", { name }));
    const result = await commands.syncMaterialize(path);
    settle(path);
    if (result.status !== "ok") {
      onStatus(t("cloud.sync_failed", { name }));
      return false;
    }
    switch (result.data) {
      case "done":
      case "already_there":
        onStatus("");
        return true;
      case "not_found":
        onStatus(t("cloud.sync_missing", { name }));
        return false;
      default:
        onStatus(t("cloud.sync_failed", { name }));
        return false;
    }
  };

  const evict = async (path: string, name: string) => {
    const result = await commands.syncEvict(path);
    if (result.status !== "ok") {
      onStatus(t("cloud.sync_evict_failed", { name }));
      return;
    }
    switch (result.data) {
      case "done":
      case "already_there":
        onStatus(t("cloud.sync_evicted", { name }));
        return;
      case "changed":
        onStatus(t("cloud.sync_evict_unsynced", { name }));
        return;
      case "not_found":
        onStatus(t("cloud.sync_evict_not_synced", { name }));
        return;
      default:
        onStatus(t("cloud.sync_evict_failed", { name }));
    }
  };

  return { synced, progress, download, evict };
}
