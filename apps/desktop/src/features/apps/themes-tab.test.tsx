// @vitest-environment happy-dom
/**
 * 「应用 › 主题」（ADR-168 ⑦ / ADR-169 ④）：内置与导入的主题（可以多套）的预览、使用、从文件安装与卸载。
 *
 * 钉住五件事：列出的是 Rust 主题注册表的投影（前端不认识任何主题 id）；预览不写设置、离开就原样放回；
 * 「使用」只经设置写 `appearance.theme_family`；在用哪一套听 Rust 的（`theme_library`）；卸载按 id 卸那一套，
 * 正在用的确认框先说「会换回 Lunarca」。
 *
 * 在线目录（纵切 7a）另钉三件：`theme_market` 回 `null` 时那一块整个不出现；每一行按 Rust 给的状态摆按钮
 * （前端不比版本）；获取第一次即启用、更新说更新到哪一版，失败的码各有各的话。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, emitEvent, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { ConfirmHost } from "../../components/ConfirmHost";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { ThemesTab } from "./ThemesTab";

function builtin(id: string, name: string) {
  return {
    id,
    version: "1.0.0",
    name,
    author: "Lunarca",
    license: "AGPL-3.0-only",
    source: "builtin" as const,
    css: null,
    swatch_css: `[data-theme-swatch="${id}"]{}`,
    update_to: null,
  };
}
function theme(id: string, name: string, version = "1.0.0", update_to: string | null = null) {
  return {
    id,
    version,
    name,
    author: "Ada",
    license: "CC0-1.0",
    source: "content" as const,
    css: `@layer tokens.family{/*${id}*/}`,
    swatch_css: `[data-theme-swatch="${id}"]{}`,
    update_to,
  };
}
const LUNA = builtin("luna", "Lunarca");
const NEBULA = builtin("nebula", "Nebula");
const BAY = theme("bay", "海湾");
const DUSK = theme("dusk", "黄昏", "2.1.0");

let installed: ReturnType<typeof theme>[] = [];
function library(inUse: string, themes = installed, disabled: unknown[] = []) {
  installed = themes;
  setCommandReply("themeLibrary", { in_use: inUse, themes: [LUNA, NEBULA, ...themes], disabled, can_check_updates: false });
}

function show() {
  return render(withI18n(<ConfirmHost><ThemesTab /></ConfirmHost>));
}

const rowButton = (action: string, name: string) =>
  screen.findByRole("button", { name: t("workspace:apps.rowAction", { action: t(action), name }) });

beforeEach(() => {
  resetBindingsDouble();
  document.documentElement.dataset.themeFamily = "luna";
  library("luna", []);
  setCommandReply("setSetting", { status: "ok", data: { kind: "str", value: "nebula" } });
  setCommandReply("uninstallThemePackage", { status: "ok", data: null });
  // `theme_market` 返回普通值：`null` = 严格本地 / 未登录 / 服务端没发布主题目录，「获取更多主题」不出现。
  setCommandReply("themeMarket", null);
});
afterEach(cleanup);

describe("「应用 › 主题」", () => {
  it("列出注册表里的每一套（内置用译名），在用的那套写「正在使用」，各带色块；「使用」只写主题族设置", async () => {
    library("luna", [BAY, DUSK]);
    show();
    const list = within(await screen.findByRole("list", { name: t("workspace:apps.tab.themes") }));
    expect(await list.findByText(t("settings:theme_library.builtin.luna.name"))).toBeTruthy();
    expect(list.getByText(t("settings:theme_library.builtin.nebula.name"))).toBeTruthy();
    expect(await list.findByText("海湾")).toBeTruthy();
    expect(document.querySelectorAll("[data-theme-swatch]")).toHaveLength(4);
    expect(document.getElementById("luna-theme-swatches")?.textContent).toContain('[data-theme-swatch="dusk"]');
    expect(
      list.getByText(t("workspace:apps.themes.localMeta", { author: "Ada", license: "CC0-1.0", version: "2.1.0" })),
    ).toBeTruthy();
    expect(list.getByText(t("workspace:apps.themes.inUse"))).toBeTruthy();
    fireEvent.click(await rowButton("workspace:apps.themes.use", "黄昏"));
    await waitFor(() =>
      expect(commandMock("setSetting")).toHaveBeenCalledWith("appearance.theme_family", { kind: "str", value: "dusk" }),
    );
  });

  it("装上的主题有新版本时，清单上写「可更新到 …」（PRD-18-011：对应选择面也标出；更新按钮在下面的「获取更多主题」与「已安装」）", async () => {
    library("luna", [theme("dusk", "黄昏", "2.1.0", "2.2.0")]);
    show();
    const list = within(await screen.findByRole("list", { name: t("workspace:apps.tab.themes") }));
    expect(
      await list.findByText(
        t("workspace:apps.themes.localMetaUpdate", { author: "Ada", license: "CC0-1.0", version: "2.1.0", to: "2.2.0" }),
      ),
    ).toBeTruthy();
    expect(list.queryByRole("button", { name: t("workspace:apps.rowAction", { action: t("workspace:apps.themes.market.update"), name: "黄昏" }) })).toBeNull();
  });

  it("在用哪一套听 Rust 的：设置改了就重取，不在前端自己猜", async () => {
    library("luna", [BAY]);
    show();
    await screen.findByText("海湾");
    library("bay");
    emitEvent("settingsChanged", { key: "appearance.theme_family" });
    await waitFor(() =>
      expect(within(screen.getByRole("list", { name: t("workspace:apps.tab.themes") })).getAllByRole("listitem")[2].textContent).toContain(
        t("workspace:apps.themes.inUse"),
      ),
    );
  });

  it("预览导入的主题：换根节点的族、装它那段样式，不写设置；离开这一页原样放回", async () => {
    library("luna", [BAY]);
    const view = show();
    fireEvent.click(await rowButton("workspace:apps.themes.tryOn", "海湾"));
    expect(document.documentElement.dataset.themeFamily).toBe("bay");
    expect(document.getElementById("luna-installed-theme")?.textContent).toBe(BAY.css);
    expect(screen.getByText(t("workspace:apps.themes.previewing", { name: "海湾" }))).toBeTruthy();
    expect(commandMock("setSetting")).not.toHaveBeenCalled();
    view.unmount();
    expect(document.documentElement.dataset.themeFamily).toBe("luna");
    expect(document.getElementById("luna-installed-theme")).toBeNull();
  });

  it("预览内置主题后点「结束预览」换回", async () => {
    show();
    fireEvent.click(await rowButton("workspace:apps.themes.tryOn", t("settings:theme_library.builtin.nebula.name")));
    expect(document.documentElement.dataset.themeFamily).toBe("nebula");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:apps.themes.stopPreview") }));
    expect(document.documentElement.dataset.themeFamily).toBe("luna");
  });

  it("从文件安装：装上就重取清单并说装上了；同版本已装、没过校验各有各的话", async () => {
    setCommandReply("installThemePackage", { status: "ok", data: BAY });
    show();
    await screen.findByRole("list", { name: t("workspace:apps.tab.themes") });
    library("bay", [BAY]);
    fireEvent.click(screen.getByRole("button", { name: t("workspace:apps.themes.installFromFile") }));
    expect(await screen.findByText(t("workspace:apps.themes.installed", { name: "海湾" }))).toBeTruthy();
    expect(await screen.findByText("海湾")).toBeTruthy();

    setCommandReply("installThemePackage", { status: "error", error: "THEME_ALREADY_INSTALLED: bay 1.0.0" });
    fireEvent.click(screen.getByRole("button", { name: t("workspace:apps.themes.installFromFile") }));
    expect(await screen.findByText(t("workspace:apps.themes.alreadyInstalled"))).toBeTruthy();

    setCommandReply("installThemePackage", { status: "error", error: "THEME_IMPORT_REJECTED: contrast" });
    fireEvent.click(screen.getByRole("button", { name: t("workspace:apps.themes.installFromFile") }));
    expect(await screen.findByText(t("workspace:apps.themes.rejected"))).toBeTruthy();
  });

  it("卸载按 id 卸那一套；正在用的确认框先说会换回 Lunarca；内置主题没有卸载", async () => {
    library("bay", [BAY, DUSK]);
    show();
    const uninstall = await rowButton("workspace:apps.themes.uninstall.action", "海湾");
    expect(
      screen.queryByRole("button", {
        name: t("workspace:apps.rowAction", {
          action: t("workspace:apps.themes.uninstall.action"),
          name: t("settings:theme_library.builtin.luna.name"),
        }),
      }),
    ).toBeNull();
    fireEvent.click(uninstall);
    expect(await screen.findByText(t("workspace:apps.themes.uninstall.descriptionInUse"))).toBeTruthy();
    const dialog = await screen.findByRole("alertdialog");
    library("luna", [DUSK]);
    fireEvent.click(within(dialog).getByRole("button", { name: t("workspace:apps.themes.uninstall.confirm") }));
    await waitFor(() => expect(commandMock("uninstallThemePackage")).toHaveBeenCalledWith("bay"));
    await waitFor(() => expect(screen.queryByText("海湾")).toBeNull());
    expect(screen.getByText("黄昏")).toBeTruthy();
  });

  it("服务端封禁的已装主题列在「已停用」：写原因、只能卸载，不在可选的那张清单里", async () => {
    const banned = { id: "bad", version: "1.0.0", name: "坏主题", author: "Eve", license: "CC0-1.0", reason: "名称误导" };
    library("luna", [BAY], [banned]);
    show();
    await screen.findByRole("heading", { name: t("workspace:apps.themes.disabled.title") });
    expect(screen.getByText(t("workspace:apps.themes.disabled.reason", { reason: "名称误导" }))).toBeTruthy();
    const list = within(screen.getByRole("list", { name: t("workspace:apps.tab.themes") }));
    expect(list.queryByText("坏主题")).toBeNull();
    expect(
      screen.queryByRole("button", {
        name: t("workspace:apps.rowAction", { action: t("workspace:apps.themes.tryOn"), name: "坏主题" }),
      }),
    ).toBeNull();
    fireEvent.click(await rowButton("workspace:apps.themes.uninstall.action", "坏主题"));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: t("workspace:apps.themes.uninstall.confirm") }));
    await waitFor(() => expect(commandMock("uninstallThemePackage")).toHaveBeenCalledWith("bad"));
  });

  it("主题目录换了一份（封禁）时不等离开页面：清单与「获取更多主题」都随 themes://changed 重读", async () => {
    library("luna", [BAY]);
    show();
    await screen.findByText("海湾");
    const banned = { ...BAY, reason: "名称误导" };
    library("luna", [], [banned]);
    emitEvent("themesChanged", null);
    expect(await screen.findByText(t("workspace:apps.themes.disabled.reason", { reason: "名称误导" }))).toBeTruthy();
    expect(commandMock("themeMarket").mock.calls.length).toBeGreaterThan(1);
  });

  describe("获取更多主题", () => {
    function market(id: string, name: string, state: string, extra: Record<string, unknown> = {}) {
      return {
        id,
        version: "1.2.0",
        name,
        author: "Ada",
        license: "CC0-1.0",
        state,
        installed_version: null,
        css: `@layer tokens.family{/*${id}*/}`,
        swatch_css: `[data-theme-swatch="${id}"]{}`,
        ...extra,
      };
    }
    const FRESH = market("com.example.fresh", "晨雾", "available");
    const OLDER = market("com.example.older", "暮色", "update_available", { installed_version: "1.1.0" });
    const HAVE = market("com.example.have", "松林", "installed");
    const FUTURE = market("com.example.future", "极光", "needs_newer_app");

    it("Rust 回 null 时整块不出现——不放空入口", async () => {
      show();
      await screen.findByRole("list", { name: t("workspace:apps.tab.themes") });
      await waitFor(() => expect(commandMock("themeMarket")).toHaveBeenCalled());
      expect(screen.queryByRole("heading", { name: t("workspace:apps.themes.market.title") })).toBeNull();
    });

    it("每行按 Rust 给的状态摆按钮：可获取、可更新（写从哪版到哪版）、已安装、需要更新 Lunarca", async () => {
      setCommandReply("themeMarket", { themes: [FRESH, OLDER, HAVE, FUTURE] });
      show();
      await screen.findByRole("heading", { name: t("workspace:apps.themes.market.title") });
      expect(await rowButton("workspace:apps.themes.market.get", "晨雾")).toBeTruthy();
      expect(await rowButton("workspace:apps.themes.market.update", "暮色")).toBeTruthy();
      expect(
        screen.getByText(
          t("workspace:apps.themes.market.updateMeta", { author: "Ada", license: "CC0-1.0", from: "1.1.0", to: "1.2.0" }),
        ),
      ).toBeTruthy();
      expect(screen.getByText(t("workspace:apps.themes.market.installed"))).toBeTruthy();
      expect(screen.getByText(t("workspace:apps.themes.market.needsNewerApp"))).toBeTruthy();
      expect(
        screen.queryByRole("button", {
          name: t("workspace:apps.rowAction", { action: t("workspace:apps.themes.market.get"), name: "极光" }),
        }),
      ).toBeNull();
      expect(document.getElementById("luna-theme-market-swatches")?.textContent).toContain(
        '[data-theme-swatch="com.example.fresh"]',
      );
    });

    it("预览在线主题不写设置；横幅的主按钮是「获取并使用」，获取后说装上了并重取两份清单", async () => {
      setCommandReply("themeMarket", { themes: [FRESH] });
      setCommandReply("installMarketTheme", { status: "ok", data: theme("com.example.fresh", "晨雾", "1.2.0") });
      show();
      fireEvent.click(await rowButton("workspace:apps.themes.tryOn", "晨雾"));
      expect(document.documentElement.dataset.themeFamily).toBe("com.example.fresh");
      expect(commandMock("setSetting")).not.toHaveBeenCalled();
      fireEvent.click(screen.getByRole("button", { name: t("workspace:apps.themes.market.getAndUse") }));
      await waitFor(() => expect(commandMock("installMarketTheme")).toHaveBeenCalledWith("com.example.fresh"));
      expect(await screen.findByText(t("workspace:apps.themes.installed", { name: "晨雾" }))).toBeTruthy();
      expect(document.documentElement.dataset.themeFamily).toBe("luna");
      expect(commandMock("themeMarket").mock.calls.length).toBeGreaterThan(1);
    });

    it("「获取更多主题」没有自己的「检查更新」：手动入口只在「应用」页头一处（ADR-170 ③）", async () => {
      setCommandReply("themeMarket", { themes: [FRESH] });
      show();
      await screen.findByText(t("workspace:apps.themes.market.title"));
      expect(screen.queryByRole("button", { name: t("workspace:apps.updates.check") })).toBeNull();
    });

    it("更新说更新到哪一版；目录已不可用、客户端太旧各有各的话", async () => {
      setCommandReply("themeMarket", { themes: [OLDER, FRESH] });
      setCommandReply("installMarketTheme", { status: "ok", data: theme("com.example.older", "暮色", "1.2.0") });
      show();
      fireEvent.click(await rowButton("workspace:apps.themes.market.update", "暮色"));
      expect(
        await screen.findByText(t("workspace:apps.themes.market.updated", { name: "暮色", version: "1.2.0" })),
      ).toBeTruthy();

      setCommandReply("installMarketTheme", { status: "error", error: "THEME_MARKET_UNAVAILABLE" });
      fireEvent.click(await rowButton("workspace:apps.themes.market.get", "晨雾"));
      expect(await screen.findByText(t("workspace:apps.themes.market.unavailable"))).toBeTruthy();

      setCommandReply("installMarketTheme", { status: "error", error: "THEME_MARKET_NEEDS_UPDATE: requires 9.0.0" });
      fireEvent.click(await rowButton("workspace:apps.themes.market.get", "晨雾"));
      expect(await screen.findByRole("alert")).toHaveProperty("textContent", t("workspace:apps.themes.market.needsNewerApp"));
    });
  });
});
