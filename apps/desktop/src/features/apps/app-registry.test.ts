/**
 * 应用界面注册表与 Rust 清单一一对应（ADR-169 ⑧）。`Record<AppId, …>` 已在编译期管住「少一个 / 多一个」，
 * 这里再钉两件编译期看不见的事：顺序与清单一致（「应用」页按清单排）、应用 id 不与页签词撞
 * （两者共用 `apps://` 后面那一段）。
 */
import { describe, expect, it } from "vitest";

import { FIRST_PARTY_APP_IDS } from "../../ipc/bindings";
import { APP_UI, appUi, isAppId } from "./app-registry";
import { appDetailRoute } from "../../lib/app-route";
import { APPS_TABS, appsRouteOf } from "./apps-route";

describe("应用界面注册表", () => {
  it("与 Rust 清单同一份、同一顺序", () => {
    expect(Object.keys(APP_UI)).toEqual([...FIRST_PARTY_APP_IDS]);
    expect(isAppId("creator")).toBe(true);
    expect(isAppId("nope")).toBe(false);
    expect(appUi("nope")).toBeNull();
  });

  it("没有哪个应用叫页签词：每个应用的深链都落到它自己的详情页", () => {
    for (const id of FIRST_PARTY_APP_IDS) {
      expect(APPS_TABS as readonly string[]).not.toContain(id);
      expect(appsRouteOf(appDetailRoute(id).slice("apps://".length))).toEqual({ kind: "detail", id });
    }
  });
});
