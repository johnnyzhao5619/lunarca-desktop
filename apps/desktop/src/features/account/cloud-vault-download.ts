/**
 * 「把云端库下载到本机」的**唯一一份**流程（DRY）：账户页「云端库」、库切换菜单的「云端库」一节
 * 与无库空状态都走这里。选一个存放位置 → `sync_cloud_vault_download`（Rust 在其中建与云端库同名的
 * 文件夹作为库、登记进库清单、加入云端库，首次同步把内容拉下来；落点与命名全在 Rust，ADR-003）。
 * 选择器标题先说清会发生什么。要不要随即切过去由调用方定。
 */

import { commands } from "../../ipc/bindings";

type Translate = (key: string, options?: Record<string, unknown>) => string;

/** 选择器标题（`cloud` 命名空间的 `t`）：说清选的是存放位置、会在其中新建哪个文件夹。 */
export function pickParentTitle(name: string | null, t: Translate): string {
  return name === null
    ? t("sync.cloud_vaults.pick_parent_unnamed")
    : t("sync.cloud_vaults.pick_parent", { name });
}

export type CloudDownloadOutcome =
  /** 用户在文件夹选择器里取消了。 */
  | { kind: "cancelled" }
  /** 下载好了；`path` 是新库的路径。 */
  | { kind: "done"; path: string }
  /** 失败：`code` 是同步稳定码（`syncErrorText` 出文案）。 */
  | { kind: "failed"; code: string };

/**
 * `title` 是选择器标题（[`pickParentTitle`]）；文件夹名由 Rust 定。`onPicked` 在选好位置、开始下载时调
 * （调用方据此转「下载中」）。
 */
export async function downloadCloudVault(
  remoteVaultId: string,
  title: string,
  onPicked?: () => void,
): Promise<CloudDownloadOutcome> {
  const picked = await commands.pickFolder(title);
  if (picked.status !== "ok" || picked.data === null) {
    return { kind: "cancelled" };
  }
  onPicked?.();
  const result = await commands.syncCloudVaultDownload(remoteVaultId, picked.data);
  return result.status === "error"
    ? { kind: "failed", code: result.error }
    : { kind: "done", path: result.data };
}
