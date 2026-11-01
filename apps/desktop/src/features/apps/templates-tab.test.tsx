// @vitest-environment happy-dom
/**
 * 「应用 › 模板」（ADR-168 ⑦ / ADR-169 ⑤ ⑦，离线先行）：笔记模板是 Rust 模板注册表的投影（内置、模板包
 * 带来的、库里的），整理模板含模板包带来的，装上的模板包一节只在高级模式下能导入。钉住：「用它新建」走
 * 既有命令、建好就打开那篇；没有自己的模板时说清楚怎么加；整理模板的增删改仍在原来的管理框；导入先看
 * 确认框（未经审核 + 它会做的事），确认后凭预览时的 sha256 安装，取消什么都不装。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { ConfirmHost } from "../../components/ConfirmHost";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { NAVIGATE_EVENT } from "../../lib/navigate";
import { TemplatesTab } from "./TemplatesTab";

function recordRoutes() {
  const list: string[] = [];
  const listener = (event: Event) => {
    list.push(String((event as CustomEvent).detail));
    event.preventDefault();
  };
  window.addEventListener(NAVIGATE_EVENT, listener);
  return { list, stop: () => window.removeEventListener(NAVIGATE_EVENT, listener) };
}

const builtin = (noteType: "note" | "quick-note") => ({
  key: noteType,
  name: null,
  layer: "builtin" as const,
  note_type: noteType,
  file: null,
  pack_name: null,
  shadows: [],
});
const mine = (name: string, shadows: ("builtin" | "content")[] = []) => ({
  key: name,
  name,
  layer: "vault" as const,
  note_type: null,
  file: { layer: "vault" as const, path: `${name}.md` },
  pack_name: null,
  shadows,
});
const fromPack = (name: string) => ({
  key: name,
  name,
  layer: "content" as const,
  note_type: null,
  file: { layer: "content" as const, pack: "com.example.kit", path: `notes/${name}.md` },
  pack_name: "起步包",
  shadows: [],
});

const PACK = {
  id: "com.example.kit",
  version: "1.0.0",
  name: "起步包",
  publisher: "Ada",
  license: "CC0-1.0",
  note_templates: ["选题"],
  ai_presets: ["收紧"],
  distill_templates: [],
  prompts: 1,
  recipes: ["发布包"],
  capabilities: { uses_ai: true, transcribes: false, translates: false, creates_notes_in: ["Projects"], exports: false },
};

function show() {
  return render(withI18n(<ConfirmHost><TemplatesTab /></ConfirmHost>));
}

function advanced(on: boolean) {
  setCommandReply("listSettings", {
    status: "ok",
    data: [{ key: "settings.advanced_mode", current_value: { kind: "bool", value: on } }],
  });
}

beforeEach(() => {
  resetBindingsDouble();
  advanced(false);
  setCommandReply("noteTemplates", [builtin("note"), builtin("quick-note"), mine("周报")]);
  setCommandReply("distillUserTemplates", {
    status: "ok",
    data: [{ id: "t1", name: "访谈纪要", sections: [{ heading: "要点", guidance: "" }], full_text_default: false }],
  });
  setCommandReply("distillTemplateChoices", { status: "ok", data: [] });
  setCommandReply("templatePacks", []);
  setCommandReply("createNote", { status: "ok", data: { path: "笔记/新笔记.md" } });
  setCommandReply("createNoteFromTemplate", { status: "ok", data: { path: "笔记/周报 1.md" } });
  setCommandReply("openUserTemplatesDir", { status: "ok", data: null });
  setCommandReply("templateMarket", null);
});
afterEach(cleanup);

const useButton = (name: string) =>
  screen.findByRole("button", { name: t("workspace:apps.rowAction", { action: t("workspace:apps.templates.use"), name }) });

describe("「应用 › 模板」", () => {
  it("内置模板「用它新建」按类型建笔记，建好就打开那篇", async () => {
    const routes = recordRoutes();
    show();
    fireEvent.click(await useButton(t("workspace:template.note")));
    await waitFor(() => expect(commandMock("createNote")).toHaveBeenCalledWith("note"));
    await waitFor(() => expect(routes.list).toEqual(["notes://笔记/新笔记.md"]));
    routes.stop();
  });

  it("库里的与模板包里的模板都由 Rust 实例化（交回注册表给的那份）；「打开模板文件夹」走同一条命令", async () => {
    setCommandReply("noteTemplates", [builtin("note"), fromPack("选题"), mine("周报", ["content"])]);
    show();
    fireEvent.click(await useButton("周报"));
    await waitFor(() =>
      expect(commandMock("createNoteFromTemplate")).toHaveBeenCalledWith({ layer: "vault", path: "周报.md" }),
    );
    expect(screen.getByText(t("workspace:template.shadowsOthers"))).toBeTruthy();
    expect(screen.getByText(t("workspace:template.fromPack", { name: "起步包" }))).toBeTruthy();
    fireEvent.click(await useButton("选题"));
    await waitFor(() =>
      expect(commandMock("createNoteFromTemplate")).toHaveBeenCalledWith({
        layer: "content",
        pack: "com.example.kit",
        path: "notes/选题.md",
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: t("workspace:apps.templates.mine.openFolder") }));
    await waitFor(() => expect(commandMock("openUserTemplatesDir")).toHaveBeenCalled());
  });

  it("没有自己的模板时说清楚怎么加；新建失败时就地说，不跳走", async () => {
    setCommandReply("noteTemplates", [builtin("note"), builtin("quick-note")]);
    setCommandReply("createNote", { status: "error", error: "VAULT_IO" });
    const routes = recordRoutes();
    show();
    expect(await screen.findByText(t("workspace:apps.templates.mine.empty"))).toBeTruthy();
    fireEvent.click(await useButton(t("workspace:template.quick-note")));
    expect(await screen.findByText(t("workspace:apps.templates.createFailed"))).toBeTruthy();
    routes.stop();
    expect(routes.list).toEqual([]);
  });

  it("整理模板列出自己写的与模板包带来的，「管理整理模板」打开原来的管理框", async () => {
    setCommandReply("distillTemplateChoices", {
      status: "ok",
      data: [
        { id: "meeting", name: null, full_text_default: false, pack_name: null },
        { id: "pack:com.example.kit/weekly", name: "周记", full_text_default: false, pack_name: "起步包" },
      ],
    });
    show();
    expect(await screen.findByText("访谈纪要")).toBeTruthy();
    expect(screen.getByText("周记")).toBeTruthy();
    expect(screen.getByText(t("workspace:apps.templates.distill.sections", { count: 1 }))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("workspace:apps.templates.distill.manage") }));
    expect(await screen.findByRole("heading", { name: t("workspace:distill.manageTemplates") })).toBeTruthy();
  });

  it("没开高级模式就没有「从文件导入…」，也不出现空的模板包一节", async () => {
    show();
    await screen.findByText("访谈纪要");
    expect(screen.queryByRole("button", { name: t("workspace:apps.templates.packs.importFromFile") })).toBeNull();
    expect(screen.queryByRole("heading", { name: t("workspace:apps.templates.packs.title") })).toBeNull();
  });

  it("高级模式下导入：确认框写明未经审核与它会做的事，确认后凭预览的 sha256 安装；取消什么都不装", async () => {
    advanced(true);
    setCommandReply("pickTemplatePack", {
      status: "ok",
      data: { path: "/tmp/kit.zip", sha256: "ab".repeat(32), pack: PACK, installed_version: null },
    });
    setCommandReply("installTemplatePack", { status: "ok", data: PACK });
    show();
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:apps.templates.packs.importFromFile") }));
    let dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(t("workspace:apps.templates.packs.unreviewed"))).toBeTruthy();
    expect(within(dialog).getByText(t("workspace:apps.templates.packs.can.usesAi"))).toBeTruthy();
    expect(within(dialog).getByText(t("workspace:apps.templates.packs.can.createsNotesIn", { dir: "Projects" }))).toBeTruthy();
    expect(within(dialog).getByText(t("workspace:apps.templates.packs.recipesLater"))).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: t("common:action.cancel") }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(commandMock("installTemplatePack")).not.toHaveBeenCalled();

    setCommandReply("templatePacks", [PACK]);
    fireEvent.click(screen.getByRole("button", { name: t("workspace:apps.templates.packs.importFromFile") }));
    dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: t("workspace:apps.templates.packs.confirm") }));
    await waitFor(() => expect(commandMock("installTemplatePack")).toHaveBeenCalledWith("/tmp/kit.zip", "ab".repeat(32)));
    expect(await screen.findByText(t("workspace:apps.templates.packs.installed", { name: "起步包" }))).toBeTruthy();
    expect(
      await screen.findByText(t("workspace:apps.templates.packs.recipes", { titles: "发布包" })),
    ).toBeTruthy();
  });

  it("导入失败各有各的话；卸载先确认、按 id 卸", async () => {
    advanced(true);
    setCommandReply("templatePacks", [PACK]);
    setCommandReply("pickTemplatePack", { status: "error", error: "TEMPLATE_PACK_INCOMPATIBLE: needs >=2" });
    setCommandReply("uninstallTemplatePack", { status: "ok", data: null });
    show();
    fireEvent.click(await screen.findByRole("button", { name: t("workspace:apps.templates.packs.importFromFile") }));
    expect(await screen.findByText(t("workspace:apps.templates.packs.incompatible"))).toBeTruthy();
    fireEvent.click(
      screen.getByRole("button", {
        name: t("workspace:apps.rowAction", { action: t("workspace:apps.templates.packs.uninstall.action"), name: "起步包" }),
      }),
    );
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: t("workspace:apps.templates.packs.uninstall.confirm") }));
    await waitFor(() => expect(commandMock("uninstallTemplatePack")).toHaveBeenCalledWith("com.example.kit"));
  });
});

const CAPS = { uses_ai: false, transcribes: false, translates: false, creates_notes_in: [] as string[], exports: false };
const marketPack = (over: Record<string, unknown> = {}) => ({
  id: "com.example.studio",
  version: "1.2.0",
  name: "工作室包",
  publisher: "Example Studio",
  license: "CC-BY-4.0",
  contents: { note_templates: 3, ai_presets: 1, distill_templates: 0, prompts: 1, recipes: 0 },
  capabilities: { ...CAPS, uses_ai: true },
  bytes: 2048,
  state: "available",
  installed_version: null,
  new_capabilities: null,
  ...over,
});

describe("「获取更多模板」（纵切 7b B2）", () => {
  it("目录不可用（Rust 回 null）时这一块不出现", async () => {
    show();
    await screen.findByText(t("workspace:apps.templates.builtin.title"));
    expect(screen.queryByText(t("workspace:apps.templates.market.title"))).toBeNull();
  });

  it("获取：确认框列出它会做的事、不写「未经审核」，确认后按 id 交给 Rust（不带能力确认），装好就说并重取清单", async () => {
    setCommandReply("templateMarket", { packs: [marketPack()] });
    setCommandReply("installMarketTemplate", { status: "ok", data: { ...PACK, id: "com.example.studio", name: "工作室包", version: "1.2.0", update_to: null } });
    show();
    const get = await screen.findByRole("button", {
      name: t("workspace:apps.rowAction", { action: t("workspace:apps.templates.market.get"), name: "工作室包" }),
    });
    fireEvent.click(get);
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(t("workspace:apps.templates.packs.can.usesAi"))).toBeTruthy();
    expect(within(dialog).queryByText(t("workspace:apps.templates.packs.unreviewed"))).toBeNull();
    expect(commandMock("installMarketTemplate")).not.toHaveBeenCalled();
    const packsBefore = commandMock("templatePacks").mock.calls.length;
    fireEvent.click(within(dialog).getByRole("button", { name: t("workspace:apps.templates.market.get") }));
    await waitFor(() => expect(commandMock("installMarketTemplate")).toHaveBeenCalledWith("com.example.studio", false));
    expect(
      await screen.findByText(t("workspace:apps.templates.market.installed", { name: "工作室包", version: "1.2.0" })),
    ).toBeTruthy();
    await waitFor(() => expect(commandMock("templatePacks").mock.calls.length).toBeGreaterThan(packsBefore));
  });

  it("更新多出能力：确认框写清新版本还会做什么，确认即带着能力确认去装（PRD-18-011）", async () => {
    setCommandReply("templateMarket", {
      packs: [marketPack({
        state: "update_available",
        installed_version: "1.1.0",
        new_capabilities: { ...CAPS, creates_notes_in: ["Projects"] },
      })],
    });
    setCommandReply("installMarketTemplate", { status: "ok", data: { ...PACK, name: "工作室包", version: "1.2.0", update_to: null } });
    show();
    expect(
      await screen.findByText(
        t("workspace:apps.templates.market.updateMeta", { publisher: "Example Studio", license: "CC-BY-4.0", from: "1.1.0", to: "1.2.0" }),
      ),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", {
      name: t("workspace:apps.rowAction", { action: t("workspace:apps.templates.market.update"), name: "工作室包" }),
    }));
    const dialog = await screen.findByRole("alertdialog");
    const gains = within(dialog).getByRole("list", { name: t("workspace:apps.templates.market.gainsLead") });
    expect(gains.textContent).toContain(t("workspace:apps.templates.packs.can.createsNotesIn", { dir: "Projects" }));
    fireEvent.click(within(dialog).getByRole("button", { name: t("workspace:apps.templates.market.update") }));
    await waitFor(() => expect(commandMock("installMarketTemplate")).toHaveBeenCalledWith("com.example.studio", true));
    expect(await screen.findByText(t("workspace:apps.templates.market.updated", { name: "工作室包", version: "1.2.0" }))).toBeTruthy();
  });

  it("已装与需要新版 Lunarca 的不给按钮；下载的包对不上目录时如实说没装", async () => {
    setCommandReply("templateMarket", {
      packs: [
        marketPack(),
        marketPack({ id: "com.example.same", name: "已装包", state: "installed", installed_version: "1.2.0" }),
        marketPack({ id: "com.example.future", name: "未来包", state: "needs_newer_app" }),
      ],
    });
    setCommandReply("installMarketTemplate", { status: "error", error: "TEMPLATE_MARKET_MISMATCH: com.example.studio" });
    show();
    expect(await screen.findByText(t("workspace:apps.templates.market.isInstalled"))).toBeTruthy();
    expect(screen.getByText(t("workspace:apps.templates.market.needsNewerApp"))).toBeTruthy();
    expect(screen.getAllByRole("button", { name: new RegExp(t("workspace:apps.templates.market.get")) })).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", {
      name: t("workspace:apps.rowAction", { action: t("workspace:apps.templates.market.get"), name: "工作室包" }),
    }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: t("workspace:apps.templates.market.get") }));
    expect(await screen.findByText(t("workspace:apps.templates.market.mismatch"))).toBeTruthy();
  });

  it("这一块没有自己的「检查更新」：手动入口只在「应用」页头一处（ADR-170 ③）", async () => {
    setCommandReply("templateMarket", { packs: [marketPack()] });
    show();
    await screen.findByText(t("workspace:apps.templates.market.title"));
    expect(screen.queryByRole("button", { name: t("workspace:apps.updates.check") })).toBeNull();
    expect(commandMock("checkContentUpdates")).not.toHaveBeenCalled();
  });
});
