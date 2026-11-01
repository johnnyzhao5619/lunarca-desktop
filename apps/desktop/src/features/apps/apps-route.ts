/**
 * 「应用」目的地的路由（ADR-168 ⑦）：`apps://` 后面那一段是页签词或应用 id；应用 id 后面再跟
 * `/components` 是它的「开源组件与许可证」二级页（ADR-168 ⑤）。
 *
 * 页签词与应用 id 共用一个位置，所以它们不能重名——`app-registry.test.ts` 拿 Rust 导出的
 * `FIRST_PARTY_APP_IDS` 与这里的 `APPS_TABS` 比对钉住。
 */

export type AppsTab = "apps" | "themes" | "templates" | "installed";

/** 页签的展示顺序（ADR-168 ⑦：应用 · 主题 · 模板 · 已安装）。 */
export const APPS_TABS: readonly AppsTab[] = ["apps", "themes", "templates", "installed"];

export type AppsRoute =
  | { kind: "tab"; tab: AppsTab }
  | { kind: "detail"; id: string }
  | { kind: "components"; id: string };

/** 「开源组件与许可证」二级页的路径后缀。 */
const COMPONENTS_SUFFIX = "/components";

/** 宿主解析出的 `apps://` 之后那一段 → 落到哪一页（空 = 「应用」页签）。 */
export function appsRouteOf(selected: string | null): AppsRoute {
  if (selected === null || selected === "") return { kind: "tab", tab: "apps" };
  if (selected.endsWith(COMPONENTS_SUFFIX)) {
    return { kind: "components", id: selected.slice(0, -COMPONENTS_SUFFIX.length) };
  }
  const tab = APPS_TABS.find((value) => value !== "apps" && value === selected);
  return tab === undefined ? { kind: "detail", id: selected } : { kind: "tab", tab };
}

/** 一个应用的「开源组件与许可证」页。 */
export function appComponentsRoute(id: string): string {
  return `apps://${id}${COMPONENTS_SUFFIX}`;
}

/** 页签的路由（「应用」页签就是 `apps://` 本身）。 */
export function appsTabRoute(tab: AppsTab): string {
  return tab === "apps" ? "apps://" : `apps://${tab}`;
}
