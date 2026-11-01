/**
 * 库清单的**唯一一份**共享订阅（ADR-151 修订 2026-10-15「续二」；TDD-07 §4.2d 共享订阅模式）。
 *
 * 切换菜单、「管理库」与无库时的空状态都消费它。此前切换菜单与「管理库」各调一次
 * `listVaults`，切换菜单只在挂载与切库时刷新——在「管理库」里移除一个库之后，左下角的菜单里
 * 它还在（真机 2026-10-15）。现在 Rust 在库注册表每改写一次（注册、移除、改名、开库、绑定变化、
 * 换账户）就发一次 `vaults://changed`，这里收到即重取，所有消费者看到的是同一份。
 *
 * 「云端库（不在这台设备上）」一节：清单说可以列（已登录、同步可用，`cloud`）时取当前账户的
 * 云端库，只留 Rust 判了「可以下载到本机」的那些。云端库清单要走网络（解库名可能要取托管
 * 信封），故合并一阵变更再取，而本地清单当场重取。
 *
 * 前端零业务（ADR-003）：谁看得见、归哪一节、能不能下载，全在 Rust 下发的快照里。订阅收在
 * 模块级（首个消费者取一次并挂监听，最后一个卸载时撤掉并清空快照），不放在工作区根上——根上
 * 持有它就是每一次开库都让整个工作区重渲染（K1 的教训）。
 */

import {
  commands,
  events,
  type CloudVaultDto,
  type VaultEntryDto,
  type VaultListDto,
} from "../../ipc/bindings";
import { useStoreSelector, type SelectionEquality } from "../../hooks/use-store-selector";

export interface VaultListSnapshot {
  /** Rust 下发的库清单；`null` = 还没取到（或取不到）。 */
  list: VaultListDto | null;
  /** 当前账户不在这台设备上的云端库（只留可以下载到本机的）。 */
  cloud: CloudVaultDto[];
}

const EMPTY: VaultListSnapshot = { list: null, cloud: [] };

/** 合并一阵变更再取云端库：开一次库就有登记、认领、标记打开几次改写。 */
const CLOUD_REFRESH_DELAY_MS = 800;

type Listener = () => void;

const listeners = new Set<Listener>();
let current: VaultListSnapshot = EMPTY;
let disposers: Promise<() => void>[] = [];
/** 取数序号：晚回来的旧一次不能把新的快照盖回去。 */
let listTicket = 0;
let cloudTicket = 0;
let cloudTimer: ReturnType<typeof setTimeout> | null = null;

function publish(next: VaultListSnapshot) {
  current = next;
  for (const listener of listeners) listener();
}

async function loadCloud() {
  const ticket = ++cloudTicket;
  if (!current.list?.cloud) {
    if (current.cloud.length > 0) publish({ ...current, cloud: [] });
    return;
  }
  const result = await commands.syncCloudVaults();
  if (ticket !== cloudTicket || listeners.size === 0) return;
  const cloud = result.status === "ok" ? result.data.filter((vault) => vault.can_download) : [];
  publish({ ...current, cloud });
}

function scheduleCloud(delay: number) {
  if (cloudTimer !== null) clearTimeout(cloudTimer);
  cloudTimer = setTimeout(() => {
    cloudTimer = null;
    void loadCloud();
  }, delay);
}

async function loadList(withCloud: boolean) {
  const ticket = ++listTicket;
  const result = await commands.listVaults();
  if (ticket !== listTicket || listeners.size === 0) return;
  const list = result.status === "ok" ? result.data : null;
  const cloudTurnedOn = list?.cloud === true && current.list?.cloud !== true;
  publish({ list, cloud: list?.cloud ? current.cloud : [] });
  if (list?.cloud && (withCloud || cloudTurnedOn)) {
    scheduleCloud(cloudTurnedOn ? 0 : CLOUD_REFRESH_DELAY_MS);
  }
}

function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  if (listeners.size === 1) {
    disposers = [
      events.vaultsChanged.listen(() => void loadList(true)),
      // 换了账户：本地清单由 Rust 随后的 `vaults://changed` 带来；云端库换成新账户的。
      events.accountChanged.listen(() => scheduleCloud(0)),
    ];
    void loadList(true);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size > 0) return;
    for (const disposer of disposers) void disposer.then((dispose) => dispose());
    disposers = [];
    if (cloudTimer !== null) clearTimeout(cloudTimer);
    cloudTimer = null;
    // 没人看了就丢掉快照：下一个消费者挂上时重取，不拿一份可能已过期的先画。
    listTicket += 1;
    cloudTicket += 1;
    current = EMPTY;
  };
}

function getSnapshot(): VaultListSnapshot {
  return current;
}

/** 只取快照里的一部分，选出来的没变就不重渲染。 */
export function useVaultList<T>(
  select: (snapshot: VaultListSnapshot) => T,
  isEqual?: SelectionEquality<T>,
): T {
  return useStoreSelector(subscribe, getSnapshot, select, isEqual);
}

/** 当场重取（本地清单立即、云端库马上）：下载完一个云端库、打开切换菜单时用。 */
export function refreshVaultList(): void {
  if (listeners.size === 0) return;
  void loadList(false);
  scheduleCloud(0);
}

/** 本机库 / 列表账户的库两节（顺序即 Rust 给的呈现序：最近打开在前）。 */
export function vaultSections(list: VaultListDto | null): {
  device: VaultEntryDto[];
  account: VaultEntryDto[];
} {
  const vaults = list?.vaults ?? [];
  return {
    device: vaults.filter((vault) => vault.section === "device"),
    account: vaults.filter((vault) => vault.section === "account"),
  };
}
