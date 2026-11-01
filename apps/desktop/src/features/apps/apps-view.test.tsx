// @vitest-environment happy-dom
/**
 * 「应用」目的地（ADR-165 ⑤ / ADR-168 / PRD-19-036）：列表、详情、安装、暂时停用与卸载。
 *
 * 详情页的「启用后会多出」「权限与数据」逐条来自 Rust 清单（贡献种类与权限），这里钉住
 * 「清单里有几条就画几条」；状态与主按钮按 Rust 给的 `state` / `action` 画；要下载的安装先确认、
 * 交回的是计划里的组件 id；暂时停用与卸载先确认、确认框写明内容不动，取消就什么都不发。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, emitEvent, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type { AppDto, AppInstallPlanDto } from "../../ipc/bindings";
import { ConfirmHost } from "../../components/ConfirmHost";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { NAVIGATE_EVENT } from "../../lib/navigate";
import { AppsView } from "./AppsView";

/** 一方应用的公共字段（状态与主按钮由各用例按 Rust 的判定给出）。 */
const STORE_FACTS = { limited_by: [], version: "1.1.0", developer: "Lunarca", license: "AGPL-3.0-only" };

/** 组件的展示事实（许可证、来源、版本、路径）。 */
function component(
  id: string,
  need: "required" | "recommended" | "optional",
  presence: "missing" | "managed" | "external",
) {
  return {
    id,
    need,
    presence,
    license: id === "yt_dlp" ? "Unlicense" : "MIT",
    source: "github.com/example",
    version: presence === "managed" ? "2026.08.19" : null,
    path: presence === "missing" ? null : `/tools/${id}`,
    update_available: false,
    progress_id: id === "ffmpeg" ? null : id === "yt_dlp" ? "yt-dlp" : "deno",
  };
}

function creator(enabled: boolean): AppDto {
  return {
    ...STORE_FACTS,
    id: "creator",
    enabled,
    installed: enabled,
    state: enabled ? "installed" : "not_installed",
    action: enabled ? "open" : "install",
    size_bytes: null,
    space: "creator",
    note_types: ["video-project", "script", "swipe"],
    permissions: ["vault_folder", "microphone", "network", "ai"],
    contributions: ["space", "editor_actions", "templates", "windows", "settings"],
    components: [],
    highlights: ["pipeline", "teleprompter", "publish"],
  };
}

function linkImport(state: AppDto["state"]): AppDto {
  const installed = state !== "not_installed";
  return {
    ...STORE_FACTS,
    id: "link_import",
    enabled: state === "installed",
    installed,
    state,
    action: installed ? "open" : "install",
    size_bytes: installed ? null : 40_000_000,
    space: null,
    note_types: [],
    permissions: ["network", "external_programs", "vault_folder"],
    contributions: ["link_import", "settings"],
    components: [component("yt_dlp", "required", installed ? "managed" : "missing")],
    highlights: ["paste", "local", "sources"],
  };
}

/** 主题库里的一套（`source` 区分内置与装上的；`update_to` 由 Rust 对照在线目录算好）。 */
function themeEntry(id: string, name: string, source: "builtin" | "content", update_to: string | null = null) {
  return {
    id,
    version: "1.2.0",
    name,
    author: "Ada",
    license: "CC0-1.0",
    source,
    css: null,
    swatch_css: `[data-theme-swatch="${id}"]{}`,
    update_to,
  };
}
const LUNA_THEME = themeEntry("luna", "Lunarca", "builtin");

/** 在当前的确认框里点那颗按钮。 */
async function confirmWith(label: string) {
  const dialog = await screen.findByRole("alertdialog");
  fireEvent.click(within(dialog).getByRole("button", { name: label }));
}

/** 打开详情页页头的「···」，点里面那一项。 */
async function pickFromMore(label: string) {
  fireEvent.click(await screen.findByRole("button", { name: t("workspace:apps.more") }));
  const menu = await screen.findByRole("menu", { name: t("workspace:apps.more") });
  fireEvent.click(within(menu).getByRole("menuitem", { name: label }));
}

/** 记下这期间发出的导航（并拦住，不真的跳）。 */
function recordRoutes() {
  const list: string[] = [];
  const listener = (event: Event) => {
    list.push(String((event as CustomEvent).detail));
    event.preventDefault();
  };
  window.addEventListener(NAVIGATE_EVENT, listener);
  return { list, stop: () => window.removeEventListener(NAVIGATE_EVENT, listener) };
}

function plan(downloads: AppInstallPlanDto["downloads"]): AppInstallPlanDto {
  return { id: "creator", permissions: [], downloads, present: [], strict_local_blocked: false };
}

function show(selected: string | null) {
  return render(withI18n(<ConfirmHost><AppsView selected={selected} /></ConfirmHost>));
}

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("appsList", [creator(false)]);
  setCommandReply("appSetEnabled", { status: "ok", data: null });
  setCommandReply("appInstallPlan", { status: "ok", data: plan([]) });
  setCommandReply("appInstall", { status: "ok", data: { limited_by: [] } });
  setCommandReply("appUninstall", { status: "ok", data: null });
  // 组件页读高级模式、组件面板读路径覆盖，都经设置列表。
  setCommandReply("listSettings", { status: "ok", data: [] });
  // 主题库只有内置的；`theme_market` 为 `null` = 在线主题目录不可用。
  setCommandReply("themeLibrary", { in_use: "luna", themes: [LUNA_THEME], disabled: [], can_check_updates: false });
  setCommandReply("themeMarket", null);
  // 没装模板包；`template_market` 为 `null` = 在线模板目录不可用。
  setCommandReply("templatePacks", []);
  setCommandReply("templateMarket", null);
});
afterEach(cleanup);

describe("「应用」目的地", () => {
  it("列表列出每个一方应用，没装的行上直接给「安装」（同一张确认、同一条命令），点名字进详情", async () => {
    const routes = recordRoutes();
    show(null);
    const row = await screen.findByText(t("workspace:apps.creator.name"));
    const name = t("workspace:apps.creator.name");
    const install = screen.getByRole("button", {
      name: t("workspace:apps.rowAction", { action: t("workspace:apps.action.install"), name }),
    });
    expect(screen.queryByText(t("workspace:apps.status.not_installed"))).toBeNull();
    fireEvent.click(install);
    await confirmWith(t("workspace:apps.action.install"));
    await waitFor(() => expect(commandMock("appInstall")).toHaveBeenCalledWith("creator", []));
    // 创作有入门：从列表行装上就带去详情页，入门在那里出现。
    await waitFor(() => expect(routes.list).toEqual(["apps://creator"]));
    fireEvent.click(row);
    routes.stop();
    expect(routes.list).toEqual(["apps://creator", "apps://creator"]);
  });

  it("页签：「应用」与「已安装」，点页签走路由；「已安装」只列装了的，写版本与状态", async () => {
    setCommandReply("appsList", [creator(true), linkImport("not_installed")]);
    const routes = recordRoutes();
    const view = show(null);
    const installedTab = await screen.findByRole("tab", { name: t("workspace:apps.tab.installedCount", { count: 1 }) });
    expect(screen.getByRole("tab", { name: t("workspace:apps.tab.apps") }).getAttribute("aria-selected")).toBe("true");
    expect(screen.getByText(t("workspace:apps.link_import.name"))).toBeTruthy();
    fireEvent.click(installedTab);
    routes.stop();
    expect(routes.list).toEqual(["apps://installed"]);

    view.unmount();
    show("installed");
    await screen.findByText(t("workspace:apps.creator.name"));
    expect(screen.queryByText(t("workspace:apps.link_import.name"))).toBeNull();
    expect(screen.getByText(t("workspace:apps.meta.version", { version: "1.1.0" }))).toBeTruthy();
    expect(screen.getByText(t("workspace:apps.status.installed"))).toBeTruthy();
    expect(screen.getByRole("tab", { name: t("workspace:apps.tab.installedCount", { count: 1 }) }).getAttribute("aria-selected")).toBe("true");
  });

  it("「已安装」一个都没有时给空态，带回「应用」页签的路", async () => {
    const routes = recordRoutes();
    show("installed");
    expect(await screen.findByText(t("workspace:apps.installedEmpty.title"))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("workspace:apps.installedEmpty.browse") }));
    routes.stop();
    expect(routes.list).toEqual(["apps://"]);
  });

  it("详情按商店的写法：出处、亮点、关于（入口折进「在哪里找到它」）、权限与隐私、信息栏；未安装时主按钮是「安装」", async () => {
    show("creator");
    await screen.findByRole("heading", { name: t("workspace:apps.creator.name") });
    expect(
      screen.getByText(t("workspace:apps.byline", { developer: "Lunarca", license: "AGPL-3.0-only" })),
    ).toBeTruthy();
    for (const highlight of ["pipeline", "teleprompter", "publish"]) {
      expect(screen.getByText(t(`workspace:apps.creator.highlight.${highlight}.title`))).toBeTruthy();
    }
    expect(screen.getByText(t("workspace:apps.about.where"))).toBeTruthy();
    expect(screen.getByText(t("workspace:apps.privacy.noCollection"))).toBeTruthy();
    const info = within(screen.getByRole("complementary", { name: t("workspace:apps.info.label") }));
    expect(info.getByText("1.1.0")).toBeTruthy();
    expect(info.getByText(t("workspace:apps.info.sizeIncluded"))).toBeTruthy();
    expect(info.queryByText(t("workspace:apps.components.title"))).toBeNull();
    for (const kind of ["space", "editor_actions", "templates", "windows", "settings"]) {
      expect(screen.getByText(t(`workspace:apps.creator.contribution.${kind}`))).toBeTruthy();
    }
    for (const kind of ["vault_folder", "microphone", "network", "ai"]) {
      expect(screen.getByText(t(`workspace:apps.creator.permission.${kind}`))).toBeTruthy();
    }
    // 不用下载也先确认：「它可以：」逐条权限（Chrome 添加扩展程序的写法），不提下载。
    setCommandReply("appInstallPlan", {
      status: "ok",
      data: { ...plan([]), permissions: ["vault_folder", "microphone", "network", "ai"] },
    });
    fireEvent.click(screen.getByRole("button", { name: t("workspace:apps.action.install") }));
    const dialog = within(await screen.findByRole("alertdialog"));
    const can = within(dialog.getByRole("list", { name: t("workspace:apps.install.canLead") }));
    for (const kind of ["vault_folder", "microphone", "network", "ai"]) {
      expect(can.getByText(t(`workspace:apps.can.${kind}`))).toBeTruthy();
    }
    expect(dialog.queryByText(/MiB/)).toBeNull();
    await confirmWith(t("workspace:apps.action.install"));
    await waitFor(() => expect(commandMock("appInstall")).toHaveBeenCalledWith("creator", []));
  });

  it("要下载的安装先确认大小，取消什么都不装；确认后交回计划里的组件 id", async () => {
    setCommandReply("appInstallPlan", {
      status: "ok",
      data: plan([
        { id: "yt_dlp", source: "github.com/yt-dlp/yt-dlp", approx_download_bytes: 100, approx_disk_bytes: 1, progress_id: "yt-dlp" },
        { id: "js_runtime", source: "github.com/denoland/deno", approx_download_bytes: 100, approx_disk_bytes: 1, progress_id: "deno" },
      ]),
    });
    // 安装挂起，好看按钮上的进度；最后再放行。
    let finish: (value: unknown) => void = () => undefined;
    setCommandReply("appInstall", () => new Promise((resolve) => (finish = resolve)));
    show("creator");
    const install = await screen.findByRole("button", { name: t("workspace:apps.action.install") });
    fireEvent.click(install);
    const title = t("workspace:apps.install.title", { name: t("workspace:apps.creator.name") });
    expect(await screen.findByText(title)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("common:action.cancel") }));
    await waitFor(() => expect(screen.queryByText(title)).toBeNull());
    expect(commandMock("appInstall")).not.toHaveBeenCalled();

    fireEvent.click(install);
    await confirmWith(t("workspace:apps.action.install"));
    await waitFor(() => expect(commandMock("appInstall")).toHaveBeenCalledWith("creator", ["yt_dlp", "js_runtime"]));

    // 进度按计划给的 progress_id 认领，两件合起来算；别的下载（模型）不算进来。
    expect(await screen.findByText(t("workspace:apps.progress.downloading", { percent: 0 }))).toBeTruthy();
    emitEvent("modelDownloadProgress", { model_id: "yt-dlp", downloaded_bytes: 100, total_bytes: 100, done: true, paused: false });
    emitEvent("modelDownloadProgress", { model_id: "some-model", downloaded_bytes: 9, total_bytes: 10, done: false, paused: false });
    emitEvent("modelDownloadProgress", { model_id: "deno", downloaded_bytes: 50, total_bytes: 100, done: false, paused: false });
    expect(await screen.findByText(t("workspace:apps.progress.downloading", { percent: 75 }))).toBeTruthy();
    emitEvent("modelDownloadProgress", { model_id: "deno", downloaded_bytes: 100, total_bytes: 100, done: false, paused: false });
    expect(await screen.findByText(t("workspace:apps.progress.finishing"))).toBeTruthy();
    finish({ status: "ok", data: { limited_by: [] } });
    expect(await screen.findByRole("button", { name: t("workspace:apps.action.install") })).toBeTruthy();
  });

  it("修复用修复的话：标题与按钮都是「修复」，不重列权限", async () => {
    setCommandReply("appsList", [{ ...linkImport("installed"), state: "needs_repair", action: "repair" }]);
    setCommandReply("appInstallPlan", {
      status: "ok",
      data: {
        ...plan([
          { id: "yt_dlp", source: "github.com/yt-dlp/yt-dlp", approx_download_bytes: 1, approx_disk_bytes: 1, progress_id: "yt-dlp" },
        ]),
        id: "link_import",
        permissions: ["network", "external_programs", "vault_folder"],
      },
    });
    show("link_import");
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:apps.action.repair") }));
    const name = t("workspace:apps.link_import.name");
    expect(await screen.findByText(t("workspace:apps.repair.title", { name }))).toBeTruthy();
    expect(screen.queryByText(t("workspace:apps.install.canLead"))).toBeNull();
    await confirmWith(t("workspace:apps.action.repair"));
    await waitFor(() => expect(commandMock("appInstall")).toHaveBeenCalledWith("link_import", ["yt_dlp"]));
  });

  it("暂时停用的应用主按钮是「启用」，点了只改启用那一位", async () => {
    setCommandReply("appsList", [{ ...creator(true), enabled: false, state: "paused", action: "enable" }]);
    show("creator");
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:apps.action.enable") }));
    await waitFor(() => expect(commandMock("appSetEnabled")).toHaveBeenCalledWith("creator", true));
    expect(commandMock("appInstall")).not.toHaveBeenCalled();
  });

  it("卸载先确认，确认框写明内容不动；确认才卸载", async () => {
    setCommandReply("appsList", [creator(true)]);
    show("creator");
    // 卸载与暂时停用都在「···」里，页头只留一个主按钮（ADR-168 ④）。
    expect(screen.queryByRole("button", { name: t("workspace:apps.uninstall.action") })).toBeNull();
    await pickFromMore(t("workspace:apps.uninstall.action"));
    expect(await screen.findByText(t("workspace:apps.uninstall.description"))).toBeTruthy();
    await confirmWith(t("workspace:apps.uninstall.confirm"));
    await waitFor(() => expect(commandMock("appUninstall")).toHaveBeenCalledWith("creator"));
  });

  it("暂时停用先确认，确认框写明内容不动；取消什么都不发，确认才停用", async () => {
    setCommandReply("appsList", [creator(true)]);
    show("creator");
    await pickFromMore(t("workspace:apps.disable.action"));
    const name = t("workspace:apps.creator.name");
    expect(await screen.findByText(t("workspace:apps.disable.description", { name }))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("common:action.cancel") }));
    await waitFor(() => expect(screen.queryByText(t("workspace:apps.disable.description", { name }))).toBeNull());
    expect(commandMock("appSetEnabled")).not.toHaveBeenCalled();

    await pickFromMore(t("workspace:apps.disable.action"));
    await confirmWith(t("workspace:apps.disable.confirm"));
    await waitFor(() => expect(commandMock("appSetEnabled")).toHaveBeenCalledWith("creator", false));
  });

  it("刚启用时出现入门：选常发的渠道存进「我的渠道」，「打开示例项目」建好就进项目首页，可以跳过", async () => {
    setCommandReply("listSettings", { status: "ok", data: [] });
    const favorites = [
      { id: "youtube", default: true },
      { id: "bilibili", default: true },
    ];
    setCommandReply("creatorChannelFavorites", { status: "ok", data: favorites });
    setCommandReply("creatorSetChannelFavorites", { status: "ok", data: favorites });
    setCommandReply("creatorChannels", {
      status: "ok",
      data: ["youtube", "bilibili", "douyin"].map((id) => ({
        id,
        name: null,
        format: id === "douyin" ? ("short_video" as const) : ("long_video" as const),
        limits: { title_max: null, body_max: null, tags_max: null, tag_max: null, clip_max_secs: null },
        style: null,
        builtin: true,
        ai_notice: true,
      })),
    });
    setCommandReply("creatorCreateExampleProject", {
      status: "ok",
      data: { path: "Creator/示例/示例.md" } as unknown as never,
    });
    const routes: string[] = [];
    const listener = (event: Event) => {
      routes.push(String((event as CustomEvent).detail));
      event.preventDefault();
    };
    window.addEventListener(NAVIGATE_EVENT, listener);
    show("creator");
    const install = await screen.findByRole("button", { name: t("workspace:apps.action.install") });
    setCommandReply("appsList", [creator(true)]);
    fireEvent.click(install);
    await confirmWith(t("workspace:apps.action.install"));
    // 写回后由设置变更驱动重读（不乐观置位）。
    await waitFor(() => expect(commandMock("appInstall")).toHaveBeenCalledWith("creator", []));
    emitEvent("settingsChanged", { key: "apps.enabled" });
    expect(await screen.findByRole("heading", { name: t("workspace:creator.setup.title") })).toBeTruthy();
    // 常发渠道多选：按下抖音 = 「我的渠道」里再加抖音，也作新项目默认。
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:creator.channel.douyin") }));
    await waitFor(() =>
      expect(commandMock("creatorSetChannelFavorites")).toHaveBeenCalledWith([...favorites, { id: "douyin", default: true }]),
    );
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.setup.example") }));
    await waitFor(() => expect(routes).toEqual(["creator://project/Creator/示例/示例.md#overview"]));
    expect(screen.queryByRole("heading", { name: t("workspace:creator.setup.title") })).toBeNull();
    window.removeEventListener(NAVIGATE_EVENT, listener);
  });

  it("不是刚启用（本来就开着）时不出现入门", async () => {
    setCommandReply("appsList", [creator(true)]);
    show("creator");
    await screen.findByRole("heading", { name: t("workspace:apps.creator.name") });
    expect(screen.queryByRole("heading", { name: t("workspace:creator.setup.title") })).toBeNull();
  });

  it("带组件的应用：详情页默认不列组件，信息栏只写大小，并给去「开源组件与许可证」的路", async () => {
    setCommandReply("appsList", [creator(false), linkImport("not_installed")]);
    const routes = recordRoutes();
    show("link_import");
    await screen.findByRole("heading", { name: t("workspace:apps.link_import.name") });
    expect(screen.queryByText(t("workspace:apps.components.name.yt_dlp"))).toBeNull();
    expect(screen.queryByRole("region", { name: t("settings:tools.title") })).toBeNull();
    expect(screen.getByText(t("workspace:apps.link_import.legal"))).toBeTruthy();
    const info = within(screen.getByRole("complementary", { name: t("workspace:apps.info.label") }));
    fireEvent.click(info.getByRole("button", { name: t("workspace:apps.components.title") }));
    routes.stop();
    expect(routes.list).toEqual(["apps://link_import/components"]);
  });

  it("组件页逐件写明许可证、来源、在用的那一份、版本与位置，下面是组件面板", async () => {
    setCommandReply("appsList", [
      {
        ...linkImport("installed"),
        components: [
          component("yt_dlp", "required", "managed"),
          component("js_runtime", "recommended", "external"),
          component("ffmpeg", "optional", "missing"),
        ],
      },
    ]);
    setCommandReply("fetchToolStatus", { status: "ok", data: null });
    setCommandReply("fetchInstallPlan", { components: [], install_dir: "/tools" });
    setCommandReply("externalToolGuides", []);
    setCommandReply("listSettings", { status: "ok", data: [] });
    show("link_import/components");
    const list = within(
      await screen.findByRole("list", {
        name: t("workspace:apps.components.listLabel", { name: t("workspace:apps.link_import.name") }),
      }),
    );
    expect(
      list.getByText(
        t("workspace:apps.components.item", {
          name: t("workspace:apps.components.name.yt_dlp"),
          need: t("workspace:apps.components.need.required"),
        }),
      ),
    ).toBeTruthy();
    expect(list.getByText("Unlicense")).toBeTruthy();
    expect(list.getByText("2026.08.19")).toBeTruthy();
    expect(list.getByText("/tools/yt_dlp")).toBeTruthy();
    expect(list.getByText(t("workspace:apps.components.presence.managed"))).toBeTruthy();
    expect(list.getByText(t("workspace:apps.components.presence.external"))).toBeTruthy();
    expect(list.getByText(t("workspace:apps.components.presence.missing"))).toBeTruthy();
    expect(screen.getByRole("region", { name: t("settings:tools.title") })).toBeTruthy();
  });

  it("没装的应用在组件页不给逐件安装：装组件是「安装」的一部分，入口只有一个", async () => {
    setCommandReply("appsList", [linkImport("not_installed")]);
    show("link_import/components");
    const name = t("workspace:apps.link_import.name");
    expect(await screen.findByText(t("workspace:apps.components.installFirst", { name }))).toBeTruthy();
    expect(screen.queryByRole("region", { name: t("settings:tools.title") })).toBeNull();
  });

  it("每进一页都重取一次应用清单（组件可能在应用外被删、被装）", async () => {
    const view = show(null);
    await screen.findByText(t("workspace:apps.creator.name"));
    const before = commandMock("appsList").mock.calls.length;
    view.rerender(withI18n(<ConfirmHost><AppsView selected="creator" /></ConfirmHost>));
    await waitFor(() => expect(commandMock("appsList").mock.calls.length).toBeGreaterThan(before));
  });

  it("应用状态一变（如在详情页重新启用）组件面板跟着重取工具状态，不和页头说两样", async () => {
    setCommandReply("appsList", [linkImport("paused")]);
    setCommandReply("fetchToolStatus", { status: "ok", data: null });
    setCommandReply("fetchInstallPlan", { components: [], install_dir: "/tools" });
    setCommandReply("externalToolGuides", []);
    setCommandReply("listSettings", { status: "ok", data: [] });
    show("link_import/components");
    await screen.findByRole("region", { name: t("settings:tools.title") });
    await waitFor(() => expect(commandMock("fetchToolStatus")).toHaveBeenCalled());
    const before = commandMock("fetchToolStatus").mock.calls.length;

    setCommandReply("appsList", [linkImport("installed")]);
    emitEvent("settingsChanged", { key: "apps.enabled" });
    await waitFor(() => expect(commandMock("fetchToolStatus").mock.calls.length).toBeGreaterThan(before));
  });

  it("页头「检查更新」在哪个页签都在：点了才联网，查完说结论并重取清单；有新版的页签上写几项可更新（ADR-170 ③）", async () => {
    setCommandReply("appsList", [linkImport("installed")]);
    setCommandReply("appsCheckUpdates", { status: "ok", data: ["link_import"] });
    show(null);
    expect(await screen.findByText(t("workspace:apps.updates.hint"))).toBeTruthy();
    expect(commandMock("appsCheckUpdates")).not.toHaveBeenCalled();
    const before = commandMock("appsList").mock.calls.length;
    setCommandReply("appsList", [{ ...linkImport("installed"), state: "update_available", action: "update" }]);
    fireEvent.click(screen.getByRole("button", { name: t("workspace:apps.updates.check") }));
    expect(await screen.findByText(t("workspace:apps.updates.available", { count: 1 }))).toBeTruthy();
    expect(commandMock("appsList").mock.calls.length).toBeGreaterThan(before);
    expect(
      await screen.findByRole("tab", { name: t("workspace:apps.tab.installedUpdates", { count: 1, updates: 1 }) }),
    ).toBeTruthy();

    // 更新完之后这句话跟着此刻的状态走，不停在查的那一刻（真机发现）。
    setCommandReply("appsList", [linkImport("installed")]);
    emitEvent("settingsChanged", { key: "apps.enabled" });
    expect(await screen.findByText(t("workspace:apps.updates.upToDate"))).toBeTruthy();
  });

  it("严格本地下检查更新被拦：如实说不联网，不报成「请稍后再试」", async () => {
    setCommandReply("appsList", [linkImport("installed")]);
    setCommandReply("appsCheckUpdates", { status: "error", error: "STRICT_LOCAL_BLOCKED" });
    show("installed");
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:apps.updates.check") }));
    expect(await screen.findByText(t("workspace:apps.updates.strictLocal"))).toBeTruthy();
  });

  it("没有带组件的已装应用时不给「检查更新」（一方应用本身随 Lunarca 更新）", async () => {
    setCommandReply("appsList", [creator(true)]);
    show("installed");
    await screen.findByText(t("workspace:apps.creator.name"));
    expect(screen.queryByRole("button", { name: t("workspace:apps.updates.check") })).toBeNull();
  });

  it("可更新的应用主按钮是「更新」：不另弹确认，进度认领有新版的那几件", async () => {
    setCommandReply("appsList", [
      {
        ...linkImport("installed"),
        state: "update_available",
        action: "update",
        components: [{ ...component("yt_dlp", "required", "managed"), update_available: true }],
      },
    ]);
    let finish: (value: unknown) => void = () => undefined;
    setCommandReply("appUpdate", () => new Promise((resolve) => (finish = resolve)));
    show("link_import");
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:apps.action.update") }));
    await waitFor(() => expect(commandMock("appUpdate")).toHaveBeenCalledWith("link_import"));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    emitEvent("modelDownloadProgress", { model_id: "yt-dlp", downloaded_bytes: 25, total_bytes: 100, done: false, paused: false });
    expect(await screen.findByText(t("workspace:apps.progress.downloading", { percent: 25 }))).toBeTruthy();
    const before = commandMock("appsList").mock.calls.length;
    finish({ status: "ok", data: null });
    await waitFor(() => expect(commandMock("appsList").mock.calls.length).toBeGreaterThan(before));
  });

  it("链接指向不存在的应用：说清楚并给回列表的路", async () => {
    show("nope");
    expect(await screen.findByText(t("workspace:apps.notFound.title"))).toBeTruthy();
  });
});

describe("「已安装」里的主题（PRD-18-011 / 012 / 016）", () => {
  const DUSK = themeEntry("com.example.dusk", "黄昏", "content", "1.3.0");
  const BAY = themeEntry("com.example.bay", "海湾", "content");
  const BANNED = { id: "com.example.bad", version: "1.0.0", name: "坏主题", author: "Eve", license: "MIT", reason: "仿冒" };

  function library(canCheckUpdates: boolean, themes = [LUNA_THEME, DUSK, BAY], disabled: unknown[] = [BANNED]) {
    setCommandReply("themeLibrary", { in_use: "com.example.bay", themes, disabled, can_check_updates: canCheckUpdates });
  }

  beforeEach(() => {
    library(false);
    setCommandReply("installMarketTheme", { status: "ok", data: { ...DUSK, version: "1.3.0", update_to: null } });
    setCommandReply("uninstallThemePackage", { status: "ok", data: null });
  });

  it("列出装上的与已停用的主题（内置的不列），页签数字算上它们；没装应用时不给「还没有安装应用」", async () => {
    show("installed");
    const section = within(await screen.findByRole("region", { name: t("workspace:apps.tab.themes") }));
    expect(section.getByText("黄昏")).toBeTruthy();
    expect(section.getByText("海湾")).toBeTruthy();
    expect(section.queryByText("Lunarca")).toBeNull();
    expect(section.getByText(t("workspace:apps.themes.disabled.reason", { reason: "仿冒" }))).toBeTruthy();
    expect(section.getByText(t("workspace:apps.themes.inUse"))).toBeTruthy();
    expect(
      screen.getByRole("tab", { name: t("workspace:apps.tab.installedUpdates", { count: 3, updates: 1 }) }),
    ).toBeTruthy();
    expect(screen.queryByText(t("workspace:apps.installedEmpty.title"))).toBeNull();
  });

  it("可更新的那套写「可从 … 更新到 …」并给「更新」，点了按 id 交给 Rust，说更新到了哪一版", async () => {
    show("installed");
    expect(
      await screen.findByText(
        t("workspace:apps.themes.market.updateMeta", { author: "Ada", license: "CC0-1.0", from: "1.2.0", to: "1.3.0" }),
      ),
    ).toBeTruthy();
    const update = t("workspace:apps.rowAction", { action: t("workspace:apps.themes.market.update"), name: "黄昏" });
    expect(screen.queryByRole("button", { name: t("workspace:apps.rowAction", { action: t("workspace:apps.themes.market.update"), name: "海湾" }) })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: update }));
    await waitFor(() => expect(commandMock("installMarketTheme")).toHaveBeenCalledWith("com.example.dusk"));
    const updated = t("workspace:apps.themes.market.updated", { name: "黄昏", version: "1.3.0" });
    expect(await screen.findByText(updated)).toBeTruthy();

    // 主题目录随后换了一份（如封禁了它）：那句「已更新」不再是此刻的事实（2026-10-22 真机）。
    emitEvent("themesChanged", null);
    await waitFor(() => expect(screen.queryByText(updated)).toBeNull());
  });

  it("已停用的只能卸载：先确认，确认才卸", async () => {
    show("installed");
    fireEvent.click(
      await screen.findByRole("button", {
        name: t("workspace:apps.rowAction", { action: t("workspace:apps.themes.uninstall.action"), name: "坏主题" }),
      }),
    );
    expect(commandMock("uninstallThemePackage")).not.toHaveBeenCalled();
    await confirmWith(t("workspace:apps.themes.uninstall.confirm"));
    await waitFor(() => expect(commandMock("uninstallThemePackage")).toHaveBeenCalledWith("com.example.bad"));
  });

  it("「检查更新」：Rust 说在线主题目录可用时才给，主题这一半拉一遍目录；没有带组件的应用就不查应用", async () => {
    show("installed");
    await screen.findByText("黄昏");
    expect(screen.queryByRole("button", { name: t("workspace:apps.updates.check") })).toBeNull();
    cleanup();

    library(true);
    setCommandReply("checkContentUpdates", { status: "ok", data: null });
    show("installed");
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:apps.updates.check") }));
    await waitFor(() => expect(commandMock("checkContentUpdates")).toHaveBeenCalled());
    expect(commandMock("appsCheckUpdates")).not.toHaveBeenCalled();
    expect(
      await screen.findByText(t("workspace:apps.updates.availableWithDisabled", { count: 1, disabled: 1 })),
    ).toBeTruthy();
  });

  it("查完的结论：没有停用的就照常说「N 项可更新」/「都是最新版本」", async () => {
    library(true, [LUNA_THEME, BAY], []);
    setCommandReply("checkContentUpdates", { status: "ok", data: null });
    show("installed");
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:apps.updates.check") }));
    expect(await screen.findByText(t("workspace:apps.updates.upToDate"))).toBeTruthy();
  });

  it("装上的主题全被封禁、「获取更多主题」整块消失时照样能查——查一遍才知道解除了封禁（2026-10-23 Windows 真机）", async () => {
    library(true, [LUNA_THEME], [BANNED]);
    setCommandReply("themeMarket", null);
    setCommandReply("checkContentUpdates", { status: "ok", data: null });
    show("installed");
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:apps.updates.check") }));
    await waitFor(() => expect(commandMock("checkContentUpdates")).toHaveBeenCalled());
    // 有一项已停用时不能只说「都是最新版本」（2026-10-23 Windows 真机）。
    expect(await screen.findByText(t("workspace:apps.updates.upToDateWithDisabled", { disabled: 1 }))).toBeTruthy();
    expect(screen.queryByText(t("workspace:apps.updates.upToDate"))).toBeNull();
  });

  it("主题目录没查成（如断网）：说没能检查，不说「都是最新版本」（2026-10-22 断网真机）", async () => {
    library(true);
    setCommandReply("checkContentUpdates", { status: "error", error: "CONTENT_CHECK_FAILED: network" });
    show("installed");
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:apps.updates.check") }));
    expect(await screen.findByText(t("workspace:apps.updates.failed"))).toBeTruthy();
    expect(screen.queryByText(t("workspace:apps.updates.upToDate"))).toBeNull();
  });

  it("目录拉取被严格本地拦下：如实说不联网", async () => {
    library(true);
    setCommandReply("checkContentUpdates", { status: "error", error: "STRICT_LOCAL_BLOCKED" });
    show("installed");
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:apps.updates.check") }));
    expect(await screen.findByText(t("workspace:apps.updates.strictLocal"))).toBeTruthy();
  });
});

describe("「已安装」里的模板包（纵切 7b B2，PRD-18-011 / 016）", () => {
  const CAPS = { uses_ai: false, transcribes: false, translates: false, creates_notes_in: [] as string[], exports: false };
  const pack = (over: Record<string, unknown> = {}) => ({
    id: "com.example.studio",
    version: "1.1.0",
    name: "工作室包",
    publisher: "Example Studio",
    license: "CC-BY-4.0",
    note_templates: ["选题"],
    ai_presets: [],
    distill_templates: [],
    prompts: 0,
    recipes: [],
    capabilities: CAPS,
    update_to: null,
    ...over,
  });
  const listed = {
    id: "com.example.studio", version: "1.2.0", name: "工作室包", publisher: "Example Studio", license: "CC-BY-4.0",
    contents: { note_templates: 1, ai_presets: 0, distill_templates: 0, prompts: 0, recipes: 0 },
    capabilities: CAPS, bytes: 1024, state: "update_available", installed_version: "1.1.0", new_capabilities: null,
  };

  it("列出装上的模板包，页签数字算上它；可更新的写「可从 … 更新到 …」，更新走同一个确认框、按 id 交给 Rust", async () => {
    setCommandReply("templatePacks", [pack({ update_to: "1.2.0" })]);
    setCommandReply("templateMarket", { packs: [listed] });
    setCommandReply("installMarketTemplate", { status: "ok", data: pack({ version: "1.2.0" }) });
    show("installed");
    expect(
      await screen.findByText(
        t("workspace:apps.templates.market.updateMeta", { publisher: "Example Studio", license: "CC-BY-4.0", from: "1.1.0", to: "1.2.0" }),
      ),
    ).toBeTruthy();
    expect(screen.getByRole("tab", { name: t("workspace:apps.tab.installedUpdates", { count: 1, updates: 1 }) })).toBeTruthy();
    expect(screen.queryByText(t("workspace:apps.installedEmpty.title"))).toBeNull();
    fireEvent.click(screen.getByRole("button", {
      name: t("workspace:apps.rowAction", { action: t("workspace:apps.templates.market.update"), name: "工作室包" }),
    }));
    await confirmWith(t("workspace:apps.templates.market.update"));
    await waitFor(() => expect(commandMock("installMarketTemplate")).toHaveBeenCalledWith("com.example.studio", false));
  });

  it("「检查更新」问整个「应用」：不看装没装，主题与模板两份目录一次问完、只刷新一次", async () => {
    setCommandReply("themeLibrary", { in_use: "luna", themes: [LUNA_THEME], disabled: [], can_check_updates: true });
    setCommandReply("checkContentUpdates", { status: "ok", data: null });
    show(null);
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:apps.updates.check") }));
    await waitFor(() => expect(commandMock("checkContentUpdates")).toHaveBeenCalledWith(["themes", "templates"]));
    expect(commandMock("checkContentUpdates")).toHaveBeenCalledTimes(1);
  });

  it("服务器身份变了（信任锚换了）：说清没采用它的内容、去哪里核对，不报成「请稍后再试」", async () => {
    setCommandReply("themeLibrary", { in_use: "luna", themes: [LUNA_THEME], disabled: [], can_check_updates: true });
    setCommandReply("checkContentUpdates", { status: "error", error: "CONTENT_TRUST_CHANGED" });
    show("installed");
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:apps.updates.check") }));
    expect(await screen.findByText(t("workspace:apps.updates.trustChanged"))).toBeTruthy();
    expect(screen.queryByText(t("workspace:apps.updates.failed"))).toBeNull();
  });

  it("打开「应用」就请 Rust 重验目录（新鲜度与门控由 Rust 判），不等用户点", async () => {
    show(null);
    await waitFor(() => expect(commandMock("revalidateCatalogs")).toHaveBeenCalledTimes(1));
    expect(commandMock("checkContentUpdates")).not.toHaveBeenCalled();
  });
});
