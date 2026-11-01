/**
 * 一方应用的状态（ADR-165 ③ ④、ADR-168 ①）：每个窗口一份模块级订阅，随 `settings://changed`
 * （`apps.enabled`——安装、卸载、启用与暂时停用最后都以它广播）重读。
 *
 * 清单与判定都在 Rust（`luna_config::apps` → `apps_list`）：这里不维护「哪些类型归创作」的
 * 第二份名单，只回答「这个应用开着没有」「这个 `luna.type` 的专属入口此刻能不能露出来」。
 *
 * **取回之前一律按未启用算**：停用是默认态，先露出入口再收回会闪一下，而且那一拍里点得到。
 */

import { useSyncExternalStore } from "react";

import { commands, events, type AppDto, type AppId } from "../ipc/bindings";

const APPS_ENABLED_KEY = "apps.enabled";

let snapshot: readonly AppDto[] = [];
const listeners = new Set<() => void>();
/** 有订阅者时挂着的 `settings://changed` 订阅；最后一个订阅者离开即退订。 */
let unlisten: Promise<() => void> | null = null;

function publish(next: readonly AppDto[]) {
  snapshot = next;
  for (const listener of listeners) listener();
}

function load() {
  void commands.appsList().then(publish);
}

/** 现在就重取一次（「应用」页每进一页调：组件文件可能在应用外被删、被装）。 */
export function refreshApps(): void {
  load();
}

/**
 * 第一个订阅者到来时取一次并挂上变更订阅，最后一个离开时退订——下一次有人订阅会重新取，
 * 不会拿着一份没人更新的旧快照（取回之前沿用上一份，不闪）。
 */
function subscribe(onChange: () => void): () => void {
  listeners.add(onChange);
  if (unlisten === null) {
    load();
    unlisten = events.settingsChanged.listen((event) => {
      if (event.payload.key === APPS_ENABLED_KEY) load();
    });
  }
  return () => {
    listeners.delete(onChange);
    if (listeners.size === 0 && unlisten !== null) {
      void unlisten.then((dispose) => dispose());
      unlisten = null;
    }
  };
}

function getSnapshot(): readonly AppDto[] {
  return snapshot;
}

/** 全部一方应用及其启用状态（「应用」页用）。 */
export function useApps(): readonly AppDto[] {
  return useSyncExternalStore(subscribe, getSnapshot);
}

/** 这个应用开着没有（`AppId`：写错应用 id 过不了编译）。 */
export function useAppEnabled(id: AppId): boolean {
  return useApps().some((app) => app.id === id && app.enabled);
}

/**
 * 这个 `luna.type` 的**专属入口**此刻能不能露出来：不归任何应用，或者它的应用已启用。
 *
 * 只给「某种类型才有的入口」问（脚本工具、脚本 AI 组）。清单取回之前一律答「不能」——
 * 停用是默认态，先露出再收回会闪一下，而那一拍里点得到。
 */
export function useNoteTypeToolsAvailable(noteType: string | null): boolean {
  const apps = useApps();
  if (noteType === null || apps.length === 0) return false;
  const owner = apps.find((app) => app.note_types.includes(noteType));
  return owner === undefined || owner.enabled;
}

/** 启用 / 暂时停用一个已安装的应用（写回后由 `settings://changed` 驱动重读，不乐观置位）。 */
export async function setAppEnabled(id: string, enabled: boolean): Promise<boolean> {
  const result = await commands.appSetEnabled(id, enabled);
  return result.status === "ok";
}

/**
 * 安装（或修复，ADR-168 ③）：交回用户看过的那份计划里要下载的组件 id，Rust 核对后下载并启用。
 * 返回装上之后仍受限的组件 id；失败时返回稳定错误码（计划已变时界面重新取计划）。
 */
export async function installApp(
  id: string,
  confirmed: readonly string[],
): Promise<{ ok: true; limitedBy: string[] } | { ok: false; error: string }> {
  const result = await commands.appInstall(id, [...confirmed]);
  return result.status === "ok"
    ? { ok: true, limitedBy: result.data.limited_by }
    : { ok: false, error: result.error };
}

/**
 * 检查更新（ADR-168 ⑧）：一次查完所有已安装应用里 Lunarca 下载的组件，查完重取清单（「可更新」随之
 * 出现）。返回有新版的应用 id；失败时返回稳定错误码（严格本地为 `STRICT_LOCAL_BLOCKED`）。
 */
export async function checkAppUpdates(): Promise<{ ok: true; updates: string[] } | { ok: false; error: string }> {
  const result = await commands.appsCheckUpdates();
  load();
  return result.status === "ok" ? { ok: true, updates: result.data } : { ok: false, error: result.error };
}

/** 卸载（ADR-168 ④）：收起全部入口并删掉 Lunarca 下载的组件，库里的内容不动。 */
export async function uninstallApp(id: string): Promise<boolean> {
  const result = await commands.appUninstall(id);
  return result.status === "ok";
}
