/**
 * 设置中心的一层页面清单。顺序即侧栏顺序；页面归属由 Rust `SettingPage` 导出，前端只消费
 * 该契约。账户和个人词典没有注册表设置项，由专属面板承载。
 */
import { SETTINGS_PAGES } from "../../ipc/bindings";
export { SETTINGS_PAGES };

export type SettingsPageId = (typeof SETTINGS_PAGES)[number];

/**
 * **面板型页**：主体内容由页专属面板承载，故**当前投影下零设置行也照常呈现**。
 *
 * `orderedPages` 会过滤没有行的页面；这些页面由面板承载，因此必须明确登记以保持可导航。
 */
export const SETTINGS_PANEL_PAGES: readonly SettingsPageId[] = [
  "account",
  "glossary",
  "ai_services",
  "local_models",
];

/** 运行时字符串是否是合法页 id。 */
export function isSettingsPageId(value: string): value is SettingsPageId {
  return (SETTINGS_PAGES as readonly string[]).includes(value);
}

/**
 * 解析后的设置深链：落到哪一页、（可选）定位到哪个键/分节。
 *
 * 两段而不是三段——一层平铺之后「页」与「块」是同一件事，此前 `{category, group, target}`
 * 的中间那段随大类层一并退役。
 */
export interface ParsedSettingsRoute {
  page: SettingsPageId;
  target: string | null;
}

const SETTINGS_ROUTE_SCHEME = "settings://";

/**
 * 解析 `settings://<页>[/<键|分节>]`。
 *
 * 未知首段返回 `null`——深链笔误、过期文档、改名后残留的旧路由一律在此 fail closed，
 * 调用方据此忽略而不是停在空白页。
 */
export function parseSettingsRoute(route: string): ParsedSettingsRoute | null {
  if (!route.startsWith(SETTINGS_ROUTE_SCHEME)) return null;
  const [head = "", ...rest] = route.slice(SETTINGS_ROUTE_SCHEME.length).split("/");
  if (!isSettingsPageId(head)) return null;
  return { page: head, target: rest.join("/") || null };
}
