import { describe, expect, it } from "vitest";
import {
  SETTINGS_PAGES,
  SETTINGS_PANEL_PAGES,
  isSettingsPageId,
  parseSettingsRoute,
} from "./settings-information-architecture";
import { PAGE_ICONS } from "./SettingsNav";

describe("设置信息架构：一层平铺（PRD-09 §5 / ADR-120）", () => {
  it("一级导航是 12 个平铺页，顺序按用户任务（「外部工具」并入「应用 ▸ 从链接导入」，ADR-167 ⑤）", () => {
    expect(SETTINGS_PAGES).toEqual([
      "general",
      "appearance",
      "shortcuts",
      "ai_services",
      "local_models",
      "translate",
      "voice_input",
      "recording",
      "notes",
      "glossary",
      "privacy",
      "account",
    ]);
  });

  it("没有第二层：页 id 两两不同，且清单本身就是唯一事实源", () => {
    expect(new Set(SETTINGS_PAGES).size).toBe(SETTINGS_PAGES.length);
  });

  it("个人词典自成一页：它一份四路注入，挂到任一功能页下都会让另外三路的用户找不到", () => {
    // 语音输入 / 录音实时转录 / 媒体转写 / 实时字幕都读它（服务多个功能的东西不归任何一个功能）。
    expect(SETTINGS_PAGES).toContain("glossary");
    const pages: readonly string[] = SETTINGS_PAGES;
    expect(pages.indexOf("glossary")).toBeGreaterThan(pages.indexOf("recording"));
  });

  it("没有「高级」「外部工具」「插件」页：高级是显示投影；外部程序是「从链接导入」应用的组件（ADR-167 ⑤）", () => {
    expect(SETTINGS_PAGES).not.toContain("advanced");
    expect(isSettingsPageId("external_tools")).toBe(false);
    expect(SETTINGS_PAGES).not.toContain("plugins");
  });

  it("两层骨架的分组 id 一个不留：改名一步到位，不留兼容首段（ADR-046）", () => {
    for (const retired of [
      "notifications",
      "workspace",
      "speech",
      "transcribe",
      "popup_translate",
      "caption",
      "notes_editor",
      "chat",
      "knowledge",
      "storage_privacy",
      "labs",
      "system_diagnostics",
      "ai_models",
      "voice_transcribe",
      "translate_chat",
      "vault_storage",
      "system_advanced",
    ]) {
      expect(isSettingsPageId(retired), `${retired} 应已随两层骨架退役`).toBe(false);
    }
  });
});

describe("面板型页", () => {
  it("账户 / 个人词典 / 模型页零设置行也照常呈现", () => {
    // 词典的内容是用户长期维护的词条，不是任何一个键的值；模型页的三面全部由
    // ModelManagementPanel 承载，标准投影下零行——不登记 → 整页连同面板一起消失。
    expect(SETTINGS_PANEL_PAGES).toEqual([
      "account",
      "glossary",
      "ai_services",
      "local_models",
    ]);
    for (const page of SETTINGS_PANEL_PAGES) {
      expect(SETTINGS_PAGES).toContain(page);
    }
  });
});

describe("id 校验收口", () => {
  it("未知值一律拒绝", () => {
    expect(isSettingsPageId("general")).toBe(true);
    expect(isSettingsPageId("shortcuts")).toBe(true);
    expect(isSettingsPageId("glossary")).toBe(true);
    expect(isSettingsPageId("plugins")).toBe(false);
    expect(isSettingsPageId("")).toBe(false);
  });
});

describe("深链解析（settings://<页>[/<键|分节>]）", () => {
  it("只有页与目标两段——中间那段随大类层一并退役", () => {
    expect(parseSettingsRoute("settings://general")).toEqual({ page: "general", target: null });
    expect(parseSettingsRoute("settings://account")).toEqual({ page: "account", target: null });
    expect(parseSettingsRoute("settings://ai_services/ai.route_strategy")).toEqual({
      page: "ai_services",
      target: "ai.route_strategy",
    });
    expect(parseSettingsRoute("settings://privacy/privacy.strict_local")).toEqual({
      page: "privacy",
      target: "privacy.strict_local",
    });
    // 分节形态（模型面板的页签、行分节）：目标原样透传，由页面解释。
    expect(parseSettingsRoute("settings://local_models/models")?.target).toBe("models");
    expect(parseSettingsRoute("settings://recording/capture")?.target).toBe("capture");
  });

  it("未知首段与非设置协议 fail closed", () => {
    expect(parseSettingsRoute("")).toBeNull();
    expect(parseSettingsRoute("settings://")).toBeNull();
    expect(parseSettingsRoute("settings://advanced")).toBeNull();
    // 退役的旧分组首段不再解析：残留的旧深链要被改掉，而不是被悄悄兜住（ADR-046）。
    expect(parseSettingsRoute("settings://storage/privacy.strict_local")).toBeNull();
    expect(parseSettingsRoute("settings://system")).toBeNull();
    expect(parseSettingsRoute("notes://general")).toBeNull();
  });
});

describe("导航图标（真机反馈：外部工具与高级长成同一枚图标）", () => {
  it("每一页都登记了图标", () => {
    for (const page of SETTINGS_PAGES) {
      expect(PAGE_ICONS[page], `页 ${page} 没有图标`).toBeTruthy();
    }
  });

  it("图标两两不同——撞图标就是导航失效", () => {
    // 「外部工具」与「高级」曾同为 CpuIcon，侧栏上看不出区别；而 typecheck / lint /
    // 既有测试一个都不会红，只有真机截图看得见。判据下沉成测试，下次加页即刻拦住。
    const seen = new Map<unknown, string>();
    for (const page of SETTINGS_PAGES) {
      const icon = PAGE_ICONS[page];
      expect(seen.has(icon), `${page} 与 ${seen.get(icon)} 用了同一枚图标`).toBe(false);
      seen.set(icon, page);
    }
  });
});
