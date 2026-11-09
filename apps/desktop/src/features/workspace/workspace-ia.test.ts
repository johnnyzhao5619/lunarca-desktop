/// <reference types="vite/client" />
// 工作区信息架构（ADR-087 ① / PRD-07-002 / PRD-07-003 / PRD-07-057）：一份表派生全部。
//
// 这些断言守的是**结构**：一级默认顺序与贴底系统区、锚点与可配置项、二级归属与默认分区、
// 有没有侧栏、有没有编辑面、深链落位、分隔线位置。它们此前散在四张平行表里——加一个目的地
// 要记得改四处，漏掉哪一处都不会有任何东西变红。

import { describe, expect, it } from "vitest";
import {
  canvasDestination,
  defaultSectionOf,
  hasEditorSurface,
  hasSidebar,
  isAnchor,
  isHiddenByDefault,
  locationFor,
  navBadgeToRender,
  PRIMARY_SPACES,
  railDividerIndices,
  sameLocation,
  sectionLabelKey,
  sectionsOf,
  sidebarKeyOf,
  spaceLabelKey,
  SYSTEM_SPACES,
  WORKSPACE_IA,
  type WorkspaceSpace,
} from "./workspace-ia";

const LOCALES = import.meta.glob("../../i18n/locales/*/*.json", { eager: true }) as Record<
  string,
  { default: Record<string, unknown> }
>;

/** `ns:key` / `key`（默认 workspace 命名空间）→ 该 locale 下的文案取值（缺失为 `undefined`）。 */
function labelValue(locale: string, labelKey: string): unknown {
  const [ns, key] = labelKey.includes(":")
    ? [labelKey.slice(0, labelKey.indexOf(":")), labelKey.slice(labelKey.indexOf(":") + 1)]
    : ["workspace", labelKey];
  const bundle = LOCALES[`../../i18n/locales/${locale}/${ns}.json`]?.default;
  if (!bundle) {
    return undefined;
  }
  return key.split(".").reduce<unknown>((node, part) => {
    return typeof node === "object" && node !== null
      ? (node as Record<string, unknown>)[part]
      : undefined;
  }, bundle);
}

/** `ns:key` / `key`（默认 workspace 命名空间）→ 该 locale 下是否真有这条文案。 */
function labelExists(locale: string, labelKey: string): boolean {
  return labelValue(locale, labelKey) !== undefined;
}

describe("一级结构（PRD-07-002 / PRD-07-002b）", () => {
  it("活动栏默认顺序：搜索 → 笔记 → 知识库 → 速记 → 录音 → 转写 → AI 对话 → 创作 → 提词器", () => {
    expect(PRIMARY_SPACES).toEqual([
      "search",
      "notes",
      "knowledge",
      "quick_notes",
      "recordings",
      "transcripts",
      "chat",
      "creator",
      "teleprompter",
    ]);
  });

  it("只有一方应用的空间默认隐藏（创作 ADR-162 ①、提词器 ADR-176 ①；与 Rust 词表的 default_visible 同一事实）", () => {
    expect(PRIMARY_SPACES.filter((space) => isHiddenByDefault(space))).toEqual(["creator", "teleprompter"]);
  });

  it("系统维护区贴底且只有三项：应用 → 回收站 → 设置（ADR-165 ⑤）", () => {
    expect(SYSTEM_SPACES).toEqual(["apps", "trash", "settings"]);
  });

  it("退役的目的地一个都不在了（ADR-087 ①）", () => {
    // 启动概览、标签、图谱、通知、用量、动态各回到自己的宿主；「资料」这个上位空间随速记、
    // 录音、转写各自升为一级而退役——任何一个重新出现在活动栏上都是回流。
    const rail = [...PRIMARY_SPACES, ...SYSTEM_SPACES] as string[];
    for (const retired of [
      "dashboard",
      "drafts",
      "tags",
      "graph",
      "materials",
      "recording",
      "transcribe",
      "activity",
      "notifications",
      "usage",
    ]) {
      expect(rail).not.toContain(retired);
    }
  });

  it("一级项不重复，且每个空间在表里只有一行", () => {
    const spaces = WORKSPACE_IA.map((spec) => spec.space);
    expect(new Set(spaces).size).toBe(spaces.length);
  });

  it("锚点：笔记与知识库不可隐藏；速记 / 录音 / 转写 / 搜索 / AI 对话 / 创作可隐藏（PRD-07-003）", () => {
    const configurable = PRIMARY_SPACES.filter((space) => !isAnchor(space));
    expect(configurable).toEqual(["search", "quick_notes", "recordings", "transcripts", "chat", "creator", "teleprompter"]);
    for (const anchor of ["notes", "knowledge"] as const) {
      expect(isAnchor(anchor)).toBe(true);
    }
    // 系统区不参与显隐配置，按锚点回答。
    for (const system of SYSTEM_SPACES) {
      expect(isAnchor(system)).toBe(true);
    }
  });

  it("分隔线落在锚点段与非锚点段的边界上，且随顺序重算（PRD-07-003「区隔」）", () => {
    // 默认顺序：搜索 | 笔记 知识库 | 速记 录音 转写 AI 对话 → 第 1、3 项之前各一根。
    expect([...railDividerIndices(PRIMARY_SPACES, isAnchor)]).toEqual([1, 3]);
    // 用户把锚点排到最前：只剩一根。
    const reordered: WorkspaceSpace[] = [
      "notes",
      "knowledge",
      "search",
      "quick_notes",
      "recordings",
      "transcripts",
      "chat",
    ];
    expect([...railDividerIndices(reordered, isAnchor)]).toEqual([2]);
    // 系统区全是锚点：没有分隔线（区隔一直由 footer 的上边框在做）。
    expect(railDividerIndices(SYSTEM_SPACES, isAnchor).size).toBe(0);
    expect(railDividerIndices([], isAnchor).size).toBe(0);
  });
});

describe("二级结构（PRD-07-002 / TDD-07 §4）", () => {
  it("只有知识库有分区，且分区清单逐字固定", () => {
    expect(sectionsOf("knowledge").map((s) => s.id)).toEqual(["topics", "tags", "network"]);
  });

  it("其余空间都没有分区——它们的侧栏内容是视图内部结构，不是导航分区", () => {
    // 把「文件树 / 捕获胶囊 / 查询与结果 / 列表 / 历史会话」建模成 `WorkspaceSection` 就是
    // 伪分区：用户会以为那里还有第二层可切换的内容归属（PRD-07-002）。
    for (const space of [
      "search",
      "notes",
      "quick_notes",
      "recordings",
      "transcripts",
      "chat",
      "trash",
      "settings",
    ] as const) {
      expect(sectionsOf(space)).toEqual([]);
      expect(defaultSectionOf(space)).toBeNull();
    }
  });

  it("默认分区是显式给的，不是「取第一个」", () => {
    // 知识库的默认分区恰好排在第一位——默认与顺序同值是巧合，靠「取第一个」拿到的正确
    // 答案会在下一次调整分区顺序时静默变错。
    expect(defaultSectionOf("knowledge")).toBe("topics");
    expect(sectionsOf("knowledge")[0].id).toBe("topics");
  });

  it("分区 id 全局不重复（键要能唯一定位到一格）", () => {
    const all = WORKSPACE_IA.flatMap((spec) => spec.sections.map((s) => s.id));
    expect(new Set(all).size).toBe(all.length);
  });
});

describe("位置归一（`locationFor` 是唯一入口）", () => {
  it("不传分区 = 该空间的默认分区", () => {
    expect(locationFor("notes")).toEqual({ space: "notes", section: null });
    expect(locationFor("knowledge")).toEqual({ space: "knowledge", section: "topics" });
    expect(locationFor("recordings")).toEqual({ space: "recordings", section: null });
    expect(locationFor("search")).toEqual({ space: "search", section: null });
  });

  it("分区不属于该空间即落回默认——不存在「带着别的空间的分区」这种状态", () => {
    expect(locationFor("notes", "network")).toEqual({ space: "notes", section: null });
    expect(locationFor("recordings", "topics")).toEqual({ space: "recordings", section: null });
    expect(locationFor("chat", "topics")).toEqual({ space: "chat", section: null });
  });

  it("`sameLocation` 认空间与分区两项", () => {
    expect(sameLocation(locationFor("knowledge"), locationFor("knowledge", "topics"))).toBe(true);
    expect(sameLocation(locationFor("knowledge", "tags"), locationFor("knowledge"))).toBe(false);
    expect(sameLocation(locationFor("notes"), locationFor("quick_notes"))).toBe(false);
  });
});

describe("侧栏与编辑面（由同一份表派生）", () => {
  it("回收站无侧栏（主区全宽），其余都有", () => {
    expect(hasSidebar("trash")).toBe(false);
    for (const space of [
      "search",
      "notes",
      "knowledge",
      "quick_notes",
      "recordings",
      "transcripts",
      "chat",
      "creator",
      "settings",
    ] as const) {
      expect(hasSidebar(space)).toBe(true);
    }
  });

  it("侧栏分发键覆盖每一个有侧栏的位置", () => {
    const keys = WORKSPACE_IA.filter((spec) => spec.hasSidebar).flatMap((spec) =>
      spec.sections.length === 0
        ? [spec.space as string]
        : spec.sections.map((s) => `${spec.space}/${s.id}`),
    );
    expect(keys).toEqual([
      "search",
      "notes",
      "knowledge/topics",
      "knowledge/tags",
      "knowledge/network",
      "quick_notes",
      "recordings",
      "transcripts",
      "chat",
      "creator",
      "settings",
    ]);
    expect(sidebarKeyOf(locationFor("knowledge"))).toBe("knowledge/topics");
    expect(sidebarKeyOf(locationFor("notes"))).toBe("notes");
  });

  it("编辑工作面只有「笔记」与「搜索」共用（PRD-07-058）", () => {
    expect(hasEditorSurface(locationFor("notes"))).toBe(true);
    expect(hasEditorSurface(locationFor("search"))).toBe(true);
    // 速记有自己的单编辑画布，不是多标签/分屏那张桌子；录音与转写是回放视图。
    for (const location of [
      locationFor("quick_notes"),
      locationFor("knowledge"),
      locationFor("knowledge", "network"),
      locationFor("recordings"),
      locationFor("transcripts"),
      locationFor("chat"),
      locationFor("trash"),
      locationFor("settings"),
    ]) {
      expect(hasEditorSurface(location)).toBe(false);
    }
  });
});

describe("空白画布的落盘目的地（PRD-06-058 B′）", () => {
  it("「速记」里的空白画布属于速记——它与侧栏捕获胶囊落同一个地方", () => {
    expect(canvasDestination(locationFor("quick_notes"))).toBe("drafts");
  });

  it("笔记与搜索共用的那张桌子上写出来的是正式笔记", () => {
    expect(canvasDestination(locationFor("notes"))).toBe("notes");
    expect(canvasDestination(locationFor("search"))).toBe("notes");
  });

  it("反向条款：两个空间绝不能给出同一个答案", () => {
    // 若哪天它俩答成一样，「随手记去速记，动笔写去笔记」这条心智当场塌掉，而界面上
    // 看不出任何异常——只有事后翻文件树才发现东西落错了层。
    expect(canvasDestination(locationFor("quick_notes"))).not.toBe(
      canvasDestination(locationFor("notes")),
    );
  });
});

describe("名字（每个目的地只有一个名字键）", () => {
  it("表里每个 labelKey 在中英文里都真的存在", () => {
    // 活动栏与二级导航都是 `t(labelKey)` 这种**动态**取键，静态 key 守护看不到它们；
    // 漏一条的表现是界面上一整块裸 key（本仓被这类漏法咬过三次）。
    const keys = WORKSPACE_IA.flatMap((spec) => [
      spec.labelKey,
      ...spec.sections.map((s) => s.labelKey),
    ]);
    expect(keys.length).toBeGreaterThan(10);
    const missing = ["zh-CN", "en-US"].flatMap((locale) =>
      keys.filter((key) => !labelExists(locale, key)).map((key) => `${locale}: ${key}`),
    );
    expect(missing, `这些名字键没有文案：\n${missing.join("\n")}`).toEqual([]);
  });

  it("二级分区名只能取自 `nav.section.*`，不得借用别处的标题键", () => {
    // 借来的键属于**别的东西**（成熟度徽标的名字、各自视图的标题），它们各自会随自己的
    // 模块演进，二级导航就跟着漂，而 `labelExists` 只问"键在不在"——键一直都在。
    for (const spec of WORKSPACE_IA) {
      for (const section of spec.sections) {
        expect(
          section.labelKey.startsWith("nav.section."),
          `${spec.space}/${section.id} 的名字取自 ${section.labelKey}——二级分区名归 ` +
            "`nav.section.*` 自己所有，借别处的键迟早跟着那处一起改",
        ).toBe(true);
      }
    }
  });

  it("一级空间名不得借用各视图的标题键（搜索与 AI 对话是刻意的例外：它们只有一个名字）", () => {
    // 「转写」尤其危险：`transcribe:title` 也叫「转写」，借它就等于让空间名跟着回放视图的
    // 标题一起改（TDD-07 §4「名字只有一处」）。
    for (const spec of WORKSPACE_IA) {
      if (spec.space === "search" || spec.space === "chat") {
        continue;
      }
      expect(spec.labelKey.startsWith("nav."), `${spec.space} 的名字取自 ${spec.labelKey}`).toBe(
        true,
      );
    }
  });

  it("一级与二级名逐字等于 PRD-07-002 定的那几个词", () => {
    // 上一条只管"键从哪个命名空间来"，管不了取值。PRD-07-002 是**自己**定的用户可见名，
    // 写错了不会有任何东西变红——真机上就是这么漏过去的。
    const EXPECTED: Record<string, { zh: string; en: string }> = {
      "search.title": { zh: "搜索", en: "Search" },
      "nav.notes": { zh: "笔记", en: "Notes" },
      "nav.knowledge": { zh: "知识库", en: "Knowledge" },
      "nav.quickNotes": { zh: "速记", en: "Quick Notes" },
      "nav.recording": { zh: "录音", en: "Recordings" },
      "nav.transcripts": { zh: "转写", en: "Transcripts" },
      "chat:title": { zh: "AI 对话", en: "AI Chat" },
      "nav.creator": { zh: "创作", en: "Create" },
      "nav.teleprompter": { zh: "提词器", en: "Teleprompter" },
      "nav.apps": { zh: "应用", en: "Apps" },
      "nav.trash": { zh: "回收站", en: "Trash" },
      "nav.settings": { zh: "设置", en: "Settings" },
      "nav.section.topics": { zh: "主题页", en: "Topic pages" },
      "nav.section.tags": { zh: "标签", en: "Tags" },
      "nav.section.network": { zh: "知识网络", en: "Knowledge network" },
    };
    for (const spec of WORKSPACE_IA) {
      for (const key of [spec.labelKey, ...spec.sections.map((s) => s.labelKey)]) {
        const want = EXPECTED[key];
        expect(want, `${key} 不在 PRD-07-002 的清单里`).toBeDefined();
        expect(labelValue("zh-CN", key), key).toBe(want.zh);
        expect(labelValue("en-US", key), key).toBe(want.en);
      }
    }
  });

  it("名字键不重复——同一个键当两个目的地的名字就是两处必然一起改错", () => {
    const keys = WORKSPACE_IA.flatMap((spec) => [
      spec.labelKey,
      ...spec.sections.map((s) => s.labelKey),
    ]);
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("取名字的入口只有 `spaceLabelKey` / `sectionLabelKey`", () => {
    expect(spaceLabelKey("notes")).toBe("nav.notes");
    expect(sectionLabelKey("knowledge", "tags")).toBe("nav.section.tags");
    expect(() => sectionLabelKey("search", "tags" as never)).toThrow();
  });
});

describe("导航项角标（PRD-07-066）", () => {
  it("为 0 时不渲染——常驻的 `0` 是在提醒用户一件不存在的事", () => {
    expect(navBadgeToRender({ count: 0, labelKey: "draft.navPending", numeric: true })).toBeNull();
    expect(navBadgeToRender(undefined)).toBeNull();
  });

  it("> 0 时原样交回", () => {
    const badge = { count: 7, labelKey: "draft.navPending", numeric: true };
    expect(navBadgeToRender(badge)).toBe(badge);
  });

  it("负数不渲染（计数只可能来自 Rust 的 `COUNT(*)`，但判据不该指望这一点）", () => {
    expect(navBadgeToRender({ count: -1, labelKey: "draft.navPending" })).toBeNull();
  });
});

describe("旧模型零残留（ADR-046 无兼容层）", () => {
  const IA_SOURCE = Object.values(
    import.meta.glob("./workspace-ia.ts", {
      eager: true,
      import: "default",
      query: "?raw",
    }) as Record<string, string>,
  )[0] as unknown as string;

  const APP_SOURCE = Object.values(
    import.meta.glob("../../app/WorkspaceApp.tsx", {
      eager: true,
      import: "default",
      query: "?raw",
    }) as Record<string, string>,
  )[0] as unknown as string;

  it("扫描面非空", () => {
    expect(IA_SOURCE.length).toBeGreaterThan(1000);
    expect(APP_SOURCE.length).toBeGreaterThan(1000);
  });

  it("「当前对象」投影按**编辑面**判定，不按空间", () => {
    // 搜索与「笔记」共用同一片编辑面（PRD-07-058）：按 `space === "notes"` 判，
    // 会让搜索里打开的那篇笔记既不报字数、也拿不到上下文面板——而它就在主区上摊着。
    // 「速记」那一格取的是它自己的画布对象，不是文件树的选择（那两者未必是同一篇）。
    expect(APP_SOURCE).toContain("const activeDocumentPath = hasEditorSurface(location)");
    expect(APP_SOURCE).toContain("activeNoteTab(panesState)?.path ?? null");
    expect(APP_SOURCE).toContain('isAt("quick_notes")\n      ? selectedDraftPath');
    // 上下文面板与面包屑读的是**同一份**投影，不各自再判一次。
    expect(APP_SOURCE).toContain("const contextNotePath = activeDocumentPath?.endsWith");
    expect(APP_SOURCE).toContain("path={contextNotePath}");
  });

  it("二级分区导航在侧栏滚动区**之外**", () => {
    // 放进 `sidebar-body` 会让它在长文件树里滚出视野，而它是这一格唯一的换挡入口。
    const body = APP_SOURCE.slice(APP_SOURCE.indexOf("<WorkspaceSectionNav"));
    expect(body.indexOf('<div className="luna-workspace__sidebar-body">')).toBeGreaterThan(0);
    expect(APP_SOURCE).toContain(
      '<div className="luna-workspace__sidebar-body">{renderSidebar()}</div>',
    );
  });

  it("旧标识符在 IA 与工作区壳里都不再是可执行代码", () => {
    // 只看代码（注释里解释"它已退役"是说明，不是残留）。
    const code = (source: string) =>
      source
        .replace(/\/\*[\s\S]*?\*\//g, "\n")
        .split("\n")
        .filter((line) => !/^\s*(\/\/|\*)/.test(line))
        .join("\n");
    for (const [name, source] of [
      ["workspace-ia.ts", code(IA_SOURCE)],
      ["WorkspaceApp.tsx", code(APP_SOURCE)],
    ] as const) {
      for (const retired of [
        "WorkspaceTab",
        "NAV_GROUPS",
        "NAV_FOOTER_GROUP",
        "SIDEBAR_VIEWS",
        "EDITOR_SURFACE_VIEWS",
        "NOTE_CONTEXT_VIEWS",
        "hasNoteContext",
        "activeTab =",
        "setActiveTab",
        "activityFocus",
        "ActivityCenter",
        // 「资料」上位空间与它的两个分区、「笔记」的两个分区（ADR-087 2026-09-02 修订）。
        '"materials"',
        "imports_transcripts",
        "all_notes",
        "isHideable",
      ]) {
        expect(source, `${name} 仍在用旧模型的 ${retired}`).not.toContain(retired);
      }
    }
  });
});

describe("命令面板的「前往」空间清单（PRD-07-041）", () => {
  it("命令注册表本地的空间清单与本表逐字一致（注册表刻意不引入 workspace，否则成环）", async () => {
    const { COMMAND_SPACES } = await import("../command/command-registry");
    expect([...COMMAND_SPACES]).toEqual([...PRIMARY_SPACES, ...SYSTEM_SPACES]);
  });
});
