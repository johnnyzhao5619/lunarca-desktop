/**
 * 「应用」目的地的路由（ADR-168 ⑦）：`apps://` 后面那一段是页签词或应用 id。
 */
import { describe, expect, it } from "vitest";

import { APPS_TABS, appComponentsRoute, appsRouteOf, appsTabRoute } from "./apps-route";

describe("apps:// 路由", () => {
  it("空与 null 落「应用」页签；页签词落那个页签；其余都是应用 id", () => {
    expect(appsRouteOf(null)).toEqual({ kind: "tab", tab: "apps" });
    expect(appsRouteOf("")).toEqual({ kind: "tab", tab: "apps" });
    expect(appsRouteOf("installed")).toEqual({ kind: "tab", tab: "installed" });
    expect(appsRouteOf("creator")).toEqual({ kind: "detail", id: "creator" });
    // 「应用」页签就是 apps:// 本身，没有 apps://apps。
    expect(appsRouteOf("apps")).toEqual({ kind: "detail", id: "apps" });
  });

  it("应用 id 后跟 /components 是它的组件页，路由来回一致", () => {
    expect(appsRouteOf("link_import/components")).toEqual({ kind: "components", id: "link_import" });
    expect(appsRouteOf(appComponentsRoute("link_import").slice("apps://".length))).toEqual({
      kind: "components",
      id: "link_import",
    });
  });

  it("每个页签的路由解析回它自己", () => {
    for (const tab of APPS_TABS) {
      const route = appsTabRoute(tab);
      expect(appsRouteOf(route.slice("apps://".length))).toEqual({ kind: "tab", tab });
    }
  });
});
