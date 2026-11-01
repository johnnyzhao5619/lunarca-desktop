/**
 * 库这一侧的「新建库 / 打开文件夹为库 / 把云端库下载到本机」流程（DRY）。
 *
 * 「管理库」、无库时的空状态与库切换菜单的「云端库」一节都走这里：选文件夹 → 交给 Rust
 * （`create_vault_root` / `select_vault` 自带切库护栏；下载那一条的流程在
 * `account/cloud-vault-download`，账户页「云端库」同用）。判定全在 Rust（ADR-003），这里只串起
 * 两步并把结果归成三种。
 */

import { commands } from "../../ipc/bindings";
import { downloadCloudVault } from "../account/cloud-vault-download";
import { refreshVaultList } from "./vault-list-store";

export type VaultActionOutcome =
  /** 用户在文件夹选择器里取消了。 */
  | { kind: "cancelled" }
  /** 成功；`path` 是新库（或下载落点）的路径。 */
  | { kind: "done"; path: string }
  /** 失败：`code` 是 Rust 的稳定码（库错误或同步错误，由调用方按自己的表出文案）。 */
  | { kind: "failed"; code: string };

/** 选库位置时要对用户说的话（文案由调用方按 i18n 出，判定在 Rust；见 `use-vault-pick-prompts`）。 */
export interface VaultPickPrompts {
  /** 「文档」在 OneDrive 里时新建库选择器的标题：说明为什么建议放到 `suggested`。 */
  oneDriveTitle: (suggested: string) => string;
  /** 选中的位置在 OneDrive 里：提示可能与 OneDrive 双重同步冲突，返回是否仍然使用（不拦）。 */
  confirmOneDrive: () => Promise<boolean>;
}

/** 新建库的选择器标题：只有「文档」在 OneDrive 里、默认位置因此改到别处时才说明原因。 */
async function createPickTitle(prompts: VaultPickPrompts): Promise<string | null> {
  const suggested = await commands.defaultVaultPath();
  return suggested.status === "ok" && suggested.data.avoided_onedrive
    ? prompts.oneDriveTitle(suggested.data.path)
    : null;
}

/** 新建库（`create`）或把一个已有文件夹打开为库：选好即切过去（Rust 侧带全套切库护栏）。 */
export async function pickAndOpenVault(create: boolean, prompts: VaultPickPrompts): Promise<VaultActionOutcome> {
  const picked = await commands.pickFolder(create ? await createPickTitle(prompts) : null);
  if (picked.status !== "ok" || picked.data === null) {
    return { kind: "cancelled" };
  }
  const path = picked.data;
  if ((await commands.vaultPathInOnedrive(path)) === true && !(await prompts.confirmOneDrive())) {
    return { kind: "cancelled" };
  }
  const result = create ? await commands.createVaultRoot(path) : await commands.selectVault(path);
  return result.status === "error" ? { kind: "failed", code: result.error } : { kind: "done", path };
}

/**
 * 把一个不在这台设备上的云端库下载到本机（流程在 `account/cloud-vault-download`，账户页同用），
 * 成功后当场重取库清单：「云端库（不在这台设备上）」一节随即去掉它。
 */
export async function downloadCloudVaultHere(
  remoteVaultId: string,
  title: string,
  onPicked?: () => void,
): Promise<VaultActionOutcome> {
  const outcome = await downloadCloudVault(remoteVaultId, title, onPicked);
  if (outcome.kind === "done") {
    refreshVaultList();
  }
  return outcome;
}
