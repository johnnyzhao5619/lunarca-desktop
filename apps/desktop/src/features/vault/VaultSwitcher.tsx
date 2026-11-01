/**
 * 库切换器（PRD-07-064 / PRD-06-049，Obsidian Vault 切换器对标）。
 *
 * 落位状态栏**最左**（窗口左下角，Obsidian 同位）：「我现在在哪个库」与「就绪 / 字数」
 * 同级，是工作区的基础可观测信息（PRD-07-045 精神），故常驻可见而非藏进菜单深处。
 *
 * 前端零业务（ADR-003）：库清单、当前库标记、失效标记、分节、切换护栏判定全部由 Rust 下发，
 * 本组件只渲染与派发——"录音能不能切""有几件任务会暂停"这些话不在前端说第二遍。
 *
 * **库列表跟着账户走**（ADR-151 修订 2026-10-15「续二」）：菜单分三节——「本机库」（不属于任何
 * 账户，谁都看得见）、「<账户> 的库」（列表账户的库）与「云端库（不在这台设备上）」（当前账户的
 * 云端库里还没下载到这台设备的，点一下即选文件夹下载并切过去）。清单来自共享订阅
 * （`vault-list-store`），与「管理库」是同一份：在那边移除一个库，这里当场就没有它了。
 *
 * **弹层此前是第五份自绘菜单**：`role="menu"` + 手写的"点外部 / Esc 关闭"，而方向键、
 * roving tabindex 一概没有。已收编到全应用唯一的菜单实现——勾选态、分隔线、分组标题、失效
 * 标记那套呈现它本来就有，连"当前库打勾"用的都不必再是一个写死的 ✓ 字符。
 */

import { useTranslation } from "react-i18next";
import type { CloudVaultDto, VaultEntryDto } from "../../ipc/bindings";
import { DownloadIcon, HardDriveIcon, SettingsIcon } from "../../components/ui/Icon";
import { useContextMenu } from "../../components/ContextMenuHost";
import { buildMenu, heading, separator, type ContextMenuEntry } from "../../lib/context-menu";
import { syncErrorText } from "../account/sync-error";
import { pickParentTitle } from "../account/cloud-vault-download";
import { downloadCloudVaultHere } from "./vault-actions";
import { refreshVaultList, useVaultList, vaultSections } from "./vault-list-store";

interface VaultSwitcherProps {
  /** 打开管理库弹窗（宿主承载，切换器只派发）。 */
  onManage: () => void;
  /** 请求切换到某个库（护栏与过渡态由宿主统一处理）。 */
  onSwitch: (path: string) => void;
  /** 正在切换到的库名；非空即整个切换器转为过渡态。 */
  switchingTo: string | null;
  /** 下载云端库失败时的一句话（宿主放在状态栏）。 */
  onError?: (message: string) => void;
}

export function VaultSwitcher({ onManage, onSwitch, switchingTo, onError }: VaultSwitcherProps) {
  const { t } = useTranslation("workspace");
  const { t: tCloud } = useTranslation("cloud");
  const snapshot = useVaultList((current) => current);
  const contextMenu = useContextMenu();

  const { device, account } = vaultSections(snapshot.list);
  const cloud = snapshot.cloud;
  const active = snapshot.list?.vaults.find((entry) => entry.active);
  // 什么都没有时不渲染——首启引导或无库空状态正在处理这件事，此刻给一个空切换器只会让人困惑。
  if (!active && device.length === 0 && account.length === 0 && cloud.length === 0) {
    return null;
  }

  const vaultItem = (entry: VaultEntryDto): ContextMenuEntry => ({
    kind: "action" as const,
    id: entry.path,
    // 路径已不存在（外部删除/改名/卸载磁盘）：如实标记，界面不假装它还在。
    label: entry.missing ? t("vault.missingEntry", { name: entry.name }) : entry.name,
    // 两个库同名时，只有完整路径分得清是哪一个。
    title: entry.path,
    checked: entry.active,
    onSelect: () => {
      if (!entry.active) {
        onSwitch(entry.path);
      }
    },
  });

  const cloudItem = (vault: CloudVaultDto): ContextMenuEntry => ({
    kind: "action" as const,
    id: `cloud:${vault.id}`,
    label: vault.name ?? tCloud("sync.cloud_vaults.name_unreadable"),
    title: t("vault.cloudDownloadHint"),
    icon: <DownloadIcon size={13} />,
    onSelect: () => {
      void downloadCloudVaultHere(vault.id, pickParentTitle(vault.name, tCloud)).then((outcome) => {
        if (outcome.kind === "done") {
          onSwitch(outcome.path);
        } else if (outcome.kind === "failed") {
          onError?.(syncErrorText(outcome.code, tCloud));
        }
      });
    },
  });

  return (
    <div className="luna-vault-switcher">
      <button
        type="button"
        className="luna-vault-switcher__trigger"
        aria-haspopup="menu"
        disabled={switchingTo !== null}
        title={active?.path ?? undefined}
        onClick={(event) => {
          // 菜单按此刻的快照画；顺手重取一次，云端库那一节下次打开就是最新的。
          refreshVaultList();
          contextMenu.open(
            event,
            buildMenu([
              heading("device", t("vault.sectionDevice")),
              ...device.map(vaultItem),
              separator("account"),
              snapshot.list?.account
                ? heading("account-title", t("vault.sectionAccount", { account: snapshot.list.account }))
                : null,
              ...account.map(vaultItem),
              separator("cloud"),
              heading("cloud-title", t("vault.sectionCloud")),
              ...cloud.map(cloudItem),
              separator("manage"),
              {
                kind: "action" as const,
                id: "manage",
                label: t("vault.manage"),
                icon: <SettingsIcon size={13} />,
                onSelect: onManage,
              },
            ]),
          );
        }}
      >
        <HardDriveIcon size={13} />
        <span className="luna-vault-switcher__label">
          {switchingTo !== null
            ? t("vault.switching", { name: switchingTo })
            : (active?.name ?? t("vault.noneOpen"))}
        </span>
      </button>
    </div>
  );
}
