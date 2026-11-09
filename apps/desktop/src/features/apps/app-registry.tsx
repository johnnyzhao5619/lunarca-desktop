/**
 * 一方应用的界面注册表（ADR-169 ⑧）：清单在 Rust（`luna_config::apps`），界面上的那几样——图标、
 * 刚装上后的入门、组件二级页的面板——是 JSX，只能在前端，就都收在这一张表里，不在页面里按 id 分支。
 *
 * 键是 Rust 导出的 `AppId`（`FIRST_PARTY_APP_IDS`）：Rust 加一个应用而这里没登记、或这里多出一个，
 * typecheck 当场红；散在各处引用某个应用的地方也一律用 `AppId`，写错 id 同样过不了编译。
 */

import type { ComponentType, ReactNode } from "react";

import { FIRST_PARTY_APP_IDS, type AppId } from "../../ipc/bindings";
import { FilmIcon, LinkIcon, ScrollTextIcon } from "@lunarca/editor-core/Icon";
import { CreatorSetup } from "../creator/CreatorSetup";
import { LinkImportComponents } from "./LinkImportComponents";

export interface AppUi {
  /** 应用图标（列表行与详情页头）。 */
  icon: (size: number) => ReactNode;
  /** 组件二级页的面板；没有组件的应用为 `null`。 */
  Components: ComponentType<{ advancedMode: boolean }> | null;
  /** 刚启用后的入门；没有为 `null`。 */
  Setup: ComponentType<{ onDone: () => void }> | null;
}

export const APP_UI: Record<AppId, AppUi> = {
  creator: {
    icon: (size) => <FilmIcon size={size} />,
    Components: null,
    Setup: CreatorSetup,
  },
  teleprompter: {
    icon: (size) => <ScrollTextIcon size={size} />,
    Components: null,
    Setup: null,
  },
  link_import: {
    icon: (size) => <LinkIcon size={size} />,
    Components: LinkImportComponents,
    Setup: null,
  },
};

/** 这个 id 是不是一方应用（来自深链或 IPC 的字符串先过这一道）。 */
export function isAppId(id: string): id is AppId {
  return (FIRST_PARTY_APP_IDS as readonly string[]).includes(id);
}

/** 一个应用的界面；不认识的 id 为 `null`。 */
export function appUi(id: string): AppUi | null {
  return isAppId(id) ? APP_UI[id] : null;
}
