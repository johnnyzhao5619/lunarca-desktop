// @vitest-environment happy-dom
/**
 * 深链只在「这一次导航」时切页（2026-10-08 真机发现）：此前切换「显示完整设置」会重放上一次
 * 深链，把已经走开的用户拉回那一页。切完整设置时只允许重新定位目标，不允许再切页。
 */
import { createElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";

// 通用页带着「关于与更新」面板，它挂载时会发命令、订阅设置变更；不接替身就会打到真实的 Tauri 接口。
vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { ConfirmHost } from "../../components/ConfirmHost";
import { ContextMenuHost } from "../../components/ContextMenuHost";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { SettingsView } from "./SettingsView";
import { createSettingsControllerFixture } from "./settings-test-fixture";

afterEach(cleanup);

function view(controller: ReturnType<typeof createSettingsControllerFixture>, focus: { route: string; nonce: number }) {
  return withI18n(
    createElement(
      ContextMenuHost,
      null,
      createElement(
        ConfirmHost,
        null,
        createElement(SettingsView, { controller, pagePanels: {} as never, focus }),
      ),
    ),
  );
}

describe("设置深链不重放", () => {
  it("新深链切一次页；之后切完整设置只重新定位、不再切页", () => {
    const selectPage = vi.fn();
    const pages: ReturnType<typeof createSettingsControllerFixture>["pages"] = ["general", "privacy"];
    const base = { pages, activePage: "general" as const, selectPage };
    const focus = { route: "settings://privacy/privacy.strict_local", nonce: 1 };
    const { rerender } = render(view(createSettingsControllerFixture(base), focus));
    expect(selectPage).toHaveBeenCalledTimes(1);
    expect(selectPage).toHaveBeenLastCalledWith("privacy");

    // 用户随后走到别的页、再打开完整设置：同一个 focus 对象，不得再把他拉回隐私页。
    rerender(view(createSettingsControllerFixture({ ...base, activePage: "general", advancedMode: true }), focus));
    expect(selectPage).toHaveBeenCalledTimes(1);

    // 离开设置再回来（视图重新挂载）、外壳仍握着同一个深链对象：同样不得再切页。
    cleanup();
    render(view(createSettingsControllerFixture({ ...base, activePage: "general" }), focus));
    expect(selectPage).toHaveBeenCalledTimes(1);

    // 真正的新导航（新对象）照常切页。
    render(view(createSettingsControllerFixture({ ...base, advancedMode: true }), { ...focus, nonce: 2 }));
    expect(selectPage).toHaveBeenCalledTimes(2);
  });
});
