import { describe, expect, it } from "vitest";
import type { SettingSearchText } from "./settings-filter";
import type { SettingSpecDto } from "../../ipc/bindings";
import {
  advancedSearchCount,
  isModified,
  modifiedCountByPage,
  orderedPages,
  panelRoute,
  searchSpecs,
  searchPages,
  sectionsOfPage,
  specsOfPage,
  visibleSpecs,
} from "./settings-filter";
import { SETTINGS_PANEL_PAGES } from "./settings-information-architecture";
import { parseSettingError, shortcutDiagnosticKey } from "./settings-errors";

function spec(overrides: Partial<SettingSpecDto> & Pick<SettingSpecDto, "key">): SettingSpecDto {
  return {
    key: overrides.key,
    group: overrides.group ?? "appearance",
    scope: overrides.scope ?? "device",
    default_value: overrides.default_value ?? { kind: "str", value: "system" },
    current_value: overrides.current_value ?? { kind: "str", value: "system" },
    modified: overrides.modified ?? false,
    available: overrides.available ?? true,
    resettable: overrides.resettable ?? true,
    requires_restart: overrides.requires_restart ?? false,
    surface: overrides.surface ?? { kind: "row" },
    visibility: overrides.visibility ?? "standard",
    section: overrides.section ?? null,
    i18n_key: overrides.i18n_key ?? `settings:${overrides.key}`,
    desc_i18n_key: overrides.desc_i18n_key ?? `settings:${overrides.key}_desc`,
    desc_presentation: overrides.desc_presentation ?? "inline",
    control: overrides.control ?? { kind: "toggle" },
    shortcut_conflict: overrides.shortcut_conflict ?? null,
  };
}

const text: SettingSearchText = {
  label: (item) => (item.key === "appearance.theme" ? "主题" : item.key),
  description: (item) =>
    item.key === "appearance.theme" ? "跟随系统，或固定为浅色 / 深色。" : "",
  aliases: (item) => (item.key === "appearance.theme" ? "深色 暗色 夜间" : ""),
  pageLabel: (page) => (page === "general" ? "通用" : page),
  pageAliases: (page) => (page === "glossary" ? "词典 术语" : ""),
  sectionLabel: (item) => (item.section === "vault" ? "笔记库" : ""),
};

describe("settings-filter", () => {
  it("把已改动项与默认项区分开（恢复默认按钮据此出现）", () => {
    const untouched = spec({ key: "appearance.theme" });
    const changed = spec({
      key: "appearance.theme",
      modified: true, current_value: { kind: "str", value: "dark" },
    });
    expect(isModified(untouched)).toBe(false);
    expect(isModified(changed)).toBe(true);
  });

  /**
   * **面板承载的键必须搜得到**，且**不得**渲染成设置行。
   *
   * 两个方向各自坏一次：搜不到时用户搜「上传」「转写」找不到那个隐私开关，结论是"没这功能"
   * （转写同意此前是设置行、搜得到，归给算力卡片之后就丢了）；渲染成行则与承载它的面板
   * 重复成两个入口——模型分配那五个空下拉正是这么占住首屏的，而它们的唯一写入口是
   * `assign_model`，设置行里那个下拉根本写不进去。
   */
  it("面板承载的键留在可见集合里（搜得到），但不进设置行", () => {
    const specs = [
      spec({ key: "appearance.theme" }),
      spec({
        key: "provider.remote_asr_enabled",
        group: "ai_services",
        surface: { kind: "panel", route: "settings://ai_services/provider.remote_asr_enabled" },
      }),
    ];
    const visible = visibleSpecs(specs, { advancedMode: true });
    expect(visible.map((s) => s.key)).toContain("provider.remote_asr_enabled");

    const rows = sectionsOfPage(visible, "ai_services").flatMap((block) => block.items);
    expect(rows.map((s) => s.key)).not.toContain("provider.remote_asr_enabled");
  });

  it("每个面板承载的键都带着可用的深链——没有它，搜到了也去不了", () => {
    const withRoute = spec({
      key: "storage.models_root",
      surface: { kind: "panel", route: "settings://local_models/storage.models_root" },
    });
    expect(panelRoute(withRoute)).toBe("settings://local_models/storage.models_root");
    expect(panelRoute(spec({ key: "appearance.theme" }))).toBeNull();
  });

  it("剔除内部状态；译境未启用时隐藏其快捷键", () => {
    const specs = [
      spec({ key: "appearance.theme" }),
      spec({ key: "onboarding.completed", surface: { kind: "internal" } }),
      spec({ key: "shortcuts.desktop_lens", group: "shortcuts", available: false }),
    ];
    expect(visibleSpecs(specs, { advancedMode: false }).map((s) => s.key)).toEqual([
      "appearance.theme",
    ]);
    expect(visibleSpecs(specs.map((item) => ({ ...item, available: true })), { advancedMode: false }).map((s) => s.key)).toEqual([
      "appearance.theme",
      "shortcuts.desktop_lens",
    ]);
  });

  it("页按信息架构顺序，且不为空页留位", () => {
    const specs = [
      spec({ key: "appearance.theme", group: "general" }),
      spec({ key: "shortcuts.voice_input", group: "shortcuts" }),
    ];
    // 顺序 = `SETTINGS_PAGES` 的顺序（general 在 shortcuts 之前）；面板型页零设置项也在场。
    expect(orderedPages(specs)).toEqual([
      "general",
      "shortcuts",
      "ai_services",
      "local_models",
      "glossary",
      "account",
    ]);
    expect(specsOfPage(specs, "shortcuts").map((s) => s.key)).toEqual(["shortcuts.voice_input"]);
  });

  it("面板型页零设置项也照常呈现（PRD-07-061）", () => {
    // 个人词典的内容是用户维护的词条而非某个键的值；模型页的分配/目录/服务由面板承载、
    // 分配槽不渲染为设置行——"不为空页留位"这条规则若照搬到它们身上，那一页就永远
    // 出不来（模型页正是这样整页消失过一次）。
    expect(orderedPages([])).toEqual([
      "ai_services",
      "local_models",
      "glossary",
      "account",
    ]);
    expect(SETTINGS_PANEL_PAGES).toContain("glossary");
    expect(SETTINGS_PANEL_PAGES).toContain("local_models");
  });

  it("按页统计已改动项数，退役的旧分组 id 不计入", () => {
    const specs = [
      spec({ key: "appearance.theme", group: "general", modified: true, current_value: { kind: "str", value: "dark" } }),
      spec({ key: "appearance.language", group: "general" }),
      spec({
        key: "shortcuts.voice_input",
        group: "shortcuts",
        modified: true, current_value: { kind: "str", value: "cmd_or_ctrl+shift+j" },
        default_value: { kind: "str", value: "cmd_or_ctrl+shift+v" },
      }),
      spec({
        key: "recording.format",
        group: "recording",
        modified: true, current_value: { kind: "str", value: "wav" },
        default_value: { kind: "str", value: "aac" },
      }),
      // 退役的旧分组 id：那一页在界面上根本不存在，不为它造一个计数。
      spec({
        key: "labs.translate_space_enabled",
        group: "labs",
        modified: true, current_value: { kind: "bool", value: true },
        default_value: { kind: "bool", value: false },
      }),
    ];
    expect(modifiedCountByPage(specs)).toEqual({ general: 1, shortcuts: 1, recording: 1 });
  });

  it("本页恢复数量消费 Rust 范围，不计入领域专属配置", () => {
    const specs = [
      spec({ key: "ai.route_strategy", group: "ai_services", modified: true }),
      spec({ key: "ai.assignment.default_llm", group: "ai_services", modified: true, resettable: false }),
    ];
    expect(modifiedCountByPage(specs)).toEqual({ ai_services: 1 });
  });

  it("渐进式披露与高级项过滤（PRD-09-072）", () => {
    const specs = [
      spec({ key: "general.simple", group: "general", visibility: "standard" }),
      spec({ key: "general.deep", group: "general", visibility: "advanced" }),
    ];
    // 标准模式只展示 standard 项
    const standard = visibleSpecs(specs, { advancedMode: false });
    expect(standard.map((s) => s.key)).toEqual(["general.simple"]);

    // 高级模式下展示全部项
    const advanced = visibleSpecs(specs, { advancedMode: true });
    expect(advanced.map((s) => s.key)).toEqual(["general.simple", "general.deep"]);

    // 标准模式搜索计算高级命中数
    const hiddenCount = advancedSearchCount(specs, "deep", text);
    expect(hiddenCount).toBe(1);
  });

  it("高级搜索提示不计入内部项或不可用功能", () => {
    const specs = [
      spec({ key: "general.deep", visibility: "advanced" }),
      spec({ key: "general.deep_unavailable", visibility: "advanced", available: false }),
      spec({ key: "general.deep_internal", visibility: "advanced", surface: { kind: "internal" } }),
    ];
    expect(advancedSearchCount(specs, "deep", text)).toBe(1);
  });

  it("搜索命中标签 / 说明 / key，空查询不返回任何项", () => {
    const specs = [
      spec({ key: "appearance.theme", group: "general" }),
      spec({ key: "appearance.language", group: "general" }),
    ];
    expect(searchSpecs(specs, "", text)).toEqual([]);
    expect(searchSpecs(specs, "主题", text).map((s) => s.key)).toEqual(["appearance.theme"]);
    expect(searchSpecs(specs, "深色", text).map((s) => s.key)).toEqual(["appearance.theme"]);
    // key 可搜——文档与错误提示里出现的正是 key。
    expect(searchSpecs(specs, "language", text).map((s) => s.key)).toEqual(["appearance.language"]);
    // 页名不再下发给行——它产出的是一条页命中（见下一个用例）。
    expect(searchSpecs(specs, "通用", text)).toEqual([]);
  });

  it("页名 / 分节名 / 同义词都参与匹配——它们正是用户眼睛看到的词", () => {
    const themed = spec({ key: "appearance.theme", group: "general" });
    const vaultRow = spec({ key: "storage.vault_root", group: "storage", section: "vault" });
    const specs = [themed, vaultRow];
    // 分节名：侧栏里那一节写着「笔记库」，搜它此前命中的全是名字里带「笔记」的别的行。
    expect(searchSpecs(specs, "笔记库", text).map((s) => s.key)).toEqual(["storage.vault_root"]);
    // 同义词：行叫「主题」，而用户会搜「深色」。
    expect(searchSpecs(specs, "深色", text).map((s) => s.key)).toEqual(["appearance.theme"]);
    // 同义词只加匹配面，不改结果形态：命中的仍是那一行本身。
    expect(searchSpecs(specs, "夜间", text).map((s) => s.key)).toEqual(["appearance.theme"]);
  });

  it("行命中与页命中互不重叠：页名绝不下发给它的行", () => {
    const pages = ["general", "glossary"] as const;
    const themed = spec({ key: "appearance.theme", group: "general" });
    // 个人词典注册表零键：遍历 specs 的搜索永远碰不到它，搜「词典」此前是零结果。
    expect(searchPages(pages, "词典", text)).toEqual(["glossary"]);
    // 页名命中给的是**页**，不是把该页十几行铺开：`general` 的页名是「通用」，
    // 而 `appearance.theme` 的标签是「主题」——搜「通用」不该把它拖进来。
    expect(searchSpecs([themed], "通用", text)).toEqual([]);
    expect(searchPages(pages, "通用", text)).toEqual(["general"]);
    // 空查询两边都不产生结果（否则一进搜索态整列页都成了结果）。
    expect(searchPages(pages, "  ", text)).toEqual([]);
    expect(searchSpecs([themed], "  ", text)).toEqual([]);
  });

  it("分节视图忠实切分注册表声明：同节连续成段，未分节页是无标题单段（PRD-09-070 ⑦）", () => {
    const sectioned = [
      spec({ key: "privacy.strict_local", group: "privacy", section: "privacy" }),
      spec({ key: "storage.vault_root", group: "privacy", section: "vault" }),
      spec({ key: "vault.delete_mode", group: "privacy", section: "vault" }),
    ];
    expect(sectionsOfPage(sectioned, "privacy")).toEqual([
      { section: "privacy", items: [sectioned[0]] },
      { section: "vault", items: [sectioned[1], sectioned[2]] },
    ]);
    // 标准投影滤掉整节（如库内目录全是高级项）后，该节自然消失，不留空段。
    expect(sectionsOfPage([sectioned[0]], "privacy").map((block) => block.section)).toEqual([
      "privacy",
    ]);
    // 未分节的页（快捷键）：单段、section 为 null——视图层据此不渲染任何小节标题。
    const flat = [
      spec({ key: "shortcuts.voice_input", group: "shortcuts" }),
      spec({ key: "shortcuts.quick_chat", group: "shortcuts" }),
    ];
    expect(sectionsOfPage(flat, "shortcuts")).toEqual([{ section: null, items: flat }]);
  });
});

describe("settings-errors", () => {
  it("稳定 code 翻成本地化文案 key，未知错误如实保留细节", () => {
    expect(parseSettingError("SETTING_RANGE: caption.max_lines out of range [1, 6]")).toEqual({
      i18nKey: "error.range",
      params: { detail: "caption.max_lines out of range [1, 6]" },
    });
    expect(parseSettingError("SETTING_SHORTCUT_CONFLICT: shortcuts.voice_input")).toEqual({
      i18nKey: "error.shortcut_conflict",
      params: { detail: "shortcuts.voice_input" },
    });
    // 落盘失败（磁盘满 / 无写权限）有自己的码：值没问题，是文件写不进去（真机 2026-10-06 此前落进 unknown）。
    expect(parseSettingError("SETTING_PERSIST: config file io failed: Permission denied (os error 13)")).toEqual({
      i18nKey: "error.persist",
      params: { detail: "config file io failed: Permission denied (os error 13)" },
    });
    const unknown = parseSettingError("Vault 未打开");
    expect(unknown.i18nKey).toBe("error.unknown");
    expect(unknown.raw).toBe("Vault 未打开");
  });

  it("快捷键诊断 code 各有专属提示", () => {
    expect(shortcutDiagnosticKey("unavailable")).toBe("shortcuts.diagnostic_unavailable");
    expect(shortcutDiagnosticKey("reserved_system")).toBe("shortcuts.diagnostic_reserved_system");
    expect(shortcutDiagnosticKey("wat")).toBe("shortcuts.diagnostic_unknown");
  });
});
