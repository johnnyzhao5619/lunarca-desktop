// 版本历史控制器（PRD-06-040 快照 / PRD-06-041 版本对比）。
//
// 面板有两种读法：**差异**（这版与现在差在哪，回滚前真正要看的）与**全文**（这版长什么样）。
// 差异由 Rust 算（`luna_vault::diff`，ADR-003），前端只渲染；两者按需取数，切到哪个取哪个。

import { useCallback, useEffect, useState } from "react";
import { commands, type LineDiffDto, type SnapshotDto } from "../../ipc/bindings";
import { vaultErrorText } from "../../lib/vault-error";

export interface VersionEntry {
  id: string;
  hash: string;
  createdMs: number;
  sizeBytes: number;
}

/** 面板呈现模式：差异（默认——回滚前该看的是"改了什么"）/ 全文。 */
/** 两种读法的**顺序**即键盘 ←/→ 的顺序，故清单与类型同源，不另抄一份。 */
export const VERSION_VIEW_MODES = ["diff", "full"] as const;

export type VersionViewMode = (typeof VERSION_VIEW_MODES)[number];

export function toVersionEntries(snapshots: SnapshotDto[]): VersionEntry[] {
  return snapshots
    .map((snapshot) => ({
      id: snapshot.id,
      hash: snapshot.hash,
      createdMs: Number.parseInt(snapshot.created_ms, 10),
      sizeBytes: Number.parseInt(snapshot.size, 10),
    }))
    .sort((a, b) => b.createdMs - a.createdMs);
}

export function useVersionHistory(path: string | null) {
  const [entries, setEntries] = useState<VersionEntry[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [viewMode, setViewMode] = useState<VersionViewMode>("diff");
  const [preview, setPreview] = useState("");
  const [diff, setDiff] = useState<LineDiffDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const refresh = useCallback(async () => {
    if (!path) {
      setEntries([]);
      setSelectedId(null);
      setPreview("");
      setDiff(null);
      return;
    }

    setLoading(true);
    const next = toVersionEntries(await commands.snapshots(path));
    setEntries(next);
    setSelectedId((current) => current ?? next[0]?.id ?? null);
    setLoading(false);
  }, [path]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (!path || !selectedId) {
      setPreview("");
      setDiff(null);
      return;
    }

    let disposed = false;
    const request =
      viewMode === "diff"
        ? commands.snapshotDiff(path, selectedId).then((result) => {
            if (disposed) {
              return;
            }
            if (result.status === "ok") {
              setDiff(result.data);
              setError(null);
              return;
            }
            setDiff(null);
            setError(vaultErrorText(result.error));
          })
        : commands.readSnapshot(path, selectedId).then((result) => {
            if (disposed) {
              return;
            }
            if (result.status === "ok") {
              setPreview(result.data);
              setError(null);
              return;
            }
            setPreview("");
            setError(vaultErrorText(result.error));
          });
    void request;

    return () => {
      disposed = true;
    };
  }, [path, selectedId, viewMode]);

  const restore = useCallback(async () => {
    if (!path || !selectedId) {
      return false;
    }
    const result = await commands.restoreSnapshot(path, selectedId);
    if (result.status === "ok") {
      setError(null);
      await refresh();
      return true;
    }
    setError(vaultErrorText(result.error));
    return false;
  }, [path, refresh, selectedId]);

  return {
    entries,
    selectedId,
    setSelectedId,
    viewMode,
    setViewMode,
    preview,
    diff,
    error,
    loading,
    refresh,
    restore,
  };
}
