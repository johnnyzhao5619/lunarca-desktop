import { describe, expect, it } from "vitest";

/**
 * 创作空间的应用内统一规范（ADR-165 ⑨）由机器守住，不靠评审记性：
 *
 * 1. **页面样式原语中性命名、不借别的页面的类名**——评审时概览与灵感库借 `luna-publish__*`
 *    搭页面，改发布页就会顺手改坏它们。各页自己的前缀只归自己用；共用的去 `luna-creator-page__*`。
 * 2. **i18n 不跨节借键**——评审时灵感库的「返回看板」借 `creator.publish.back`、B 站包装借章节
 *    和简介的键：改一处文案会悄悄改到另一处。共用词表只有阶段、下一步、页面骨架、渠道与形态名。
 * 3. **即时操作的结果就近说**——创作页不再收 `onStatus` 往全局状态栏写。
 * 4. **加载中一种样子**——不再用裸 `<p className="luna-empty">`。
 *
 * 只读 `className` 与字面量 `t("…")`，不读注释（注释里提到类名不算违规），每条都有反向夹具
 * 证明它会红。
 */

const SOURCES = import.meta.glob(["./*.tsx", "./*.ts", "!./*.test.ts", "!./*.test.tsx"], {
  eager: true,
  query: "?raw",
  import: "default",
}) as Record<string, string>;

/** 各页私有的类名前缀 → 允许用它的文件。没列在这里的前缀（按钮、输入框、页头、`luna-creator-page__` 等）是共用原语。 */
const PAGE_CLASS_OWNERS: Record<string, readonly string[]> = {
  "luna-creator__": ["CreatorBoard.tsx", "CreatorViews.tsx"],
  "luna-creator-calendar": ["CreatorCalendar.tsx"],
  "luna-creator-sidebar": ["CreatorSidebar.tsx"],
  "luna-creator-list": ["CreatorSidebar.tsx"],
  "luna-project": ["ProjectWorkspace.tsx"],
  "luna-overview": ["ProjectOverview.tsx"],
  "luna-script-banner": ["ScriptProjectBanner.tsx"],
  "luna-script": ["ScriptToolbar.tsx", "ScriptInspector.tsx", "ScriptDictation.tsx", "ProjectWorkspace.tsx"],
  "luna-recording": ["RecordingPanel.tsx"],
  "luna-publish": ["PublishView.tsx", "ChaptersSection.tsx", "PackageSection.tsx"],
  "luna-extra-request": ["ExtraRequestField.tsx"],
  "luna-new-project": ["NewProjectDialog.tsx"],
  "luna-teleprompter": ["TeleprompterPanel.tsx"],
  "luna-creator-setup": ["CreatorSetup.tsx"],
  "luna-distribute": ["DistributeView.tsx"],
  "luna-channel-picker": ["ChannelPicker.tsx"],
  "luna-channel-editor": ["ChannelEditor.tsx"],
  "luna-channel-manager": ["ChannelManager.tsx"],
  "luna-add-sources": ["AddSourcesDialog.tsx"],
  "luna-research": ["ResearchDialogs.tsx"],
};

/** `creator.*` 的各节 → 允许用它的文件。最长前缀优先；没落在任何一节里的键是看板级的散键。 */
const I18N_SECTION_OWNERS: Record<string, readonly string[]> = {
  "creator.project": ["ProjectWorkspace.tsx", "NewProjectDialog.tsx"],
  "creator.projectMenu": ["project-menu.ts"],
  "creator.addSources": ["AddSourcesDialog.tsx"],
  "creator.research": ["ResearchDialogs.tsx"],
  "creator.overview": ["ProjectOverview.tsx", "use-material-import.ts"],
  "creator.script": ["ScriptToolbar.tsx", "ScriptInspector.tsx", "ScriptProjectBanner.tsx"],
  "creator.dictation": ["ScriptDictation.tsx"],
  "creator.quotes": ["ScriptInspector.tsx"],
  "creator.shots": ["ScriptInspector.tsx"],
  "creator.recording": ["RecordingPanel.tsx"],
  "creator.teleprompter": ["TeleprompterPanel.tsx"],
  "creator.publish": ["PublishView.tsx"],
  "creator.publish.common": ["PublishView.tsx", "ChaptersSection.tsx", "PackageSection.tsx"],
  // 「〈渠道〉包装」这一步的名字：步骤列与那一节的标题是同一个词。
  "creator.publish.step": ["PublishView.tsx", "PackageSection.tsx"],
  "creator.publish.chapters": ["ChaptersSection.tsx", "publish-issues.ts"],
  "creator.publish.package": ["PackageSection.tsx", "publish-issues.ts"],
  "creator.swipeLibrary": ["SwipeLibrary.tsx", "CreatorNotePage.tsx", "CreatorSidebar.tsx"],
  "creator.calendar": ["CreatorCalendar.tsx"],
  "creator.card": ["CreatorViews.tsx"],
  "creator.table": ["CreatorViews.tsx"],
  "creator.view": ["CreatorBoard.tsx"],
  "creator.setup": ["CreatorSetup.tsx"],
  "creator.distribute": ["DistributeView.tsx"],
  "creator.channels": ["ChannelPicker.tsx", "ChannelEditor.tsx", "ChannelManager.tsx"],
  "creator.extra": ["ExtraRequestField.tsx"],
  "creator.sidebar": ["CreatorSidebar.tsx"],
};

/** 全创作空间共用的词表（谁都能用）。 */
const SHARED_SECTIONS = [
  "creator.stage",
  "creator.nextAction",
  "creator.page",
  "creator.channel",
  "creator.channelFormat",
  "creator.channelNotice",
];

function stripComments(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/.*$/gm, "$1");
}

/** 一个文件里 `className` 用到的 `luna-*` 类名。 */
function classNamesOf(source: string): string[] {
  const code = stripComments(source);
  const attrs = code.match(/className=(?:"[^"]*"|\{[^}]*\})/g) ?? [];
  return attrs.flatMap((attr) => attr.match(/luna-[a-z0-9]+(?:-[a-z0-9]+)*(?:__[a-z0-9-]+)?/g) ?? []);
}

/** 一个文件里字面量 `t("creator.…")`（含 `workspace:` 前缀与模板串的静态部分）。 */
function creatorKeysOf(source: string): string[] {
  const code = stripComments(source);
  return [...code.matchAll(/\bt\(\s*["`](?:workspace:)?(creator\.[A-Za-z0-9_.]+)/g)].map((match) =>
    match[1]!.replace(/\.$/, ""),
  );
}

function longestPrefix(value: string, owners: Record<string, readonly string[]>, sep: string): string | null {
  return (
    Object.keys(owners)
      .filter((prefix) => value === prefix || value.startsWith(prefix.endsWith("__") ? prefix : `${prefix}${sep}`))
      .sort((a, b) => b.length - a.length)[0] ?? null
  );
}

function classViolations(file: string, source: string): string[] {
  return classNamesOf(source).flatMap((name) => {
    const prefix = longestPrefix(name, PAGE_CLASS_OWNERS, "_") ?? longestPrefix(name, PAGE_CLASS_OWNERS, "-");
    return prefix && !PAGE_CLASS_OWNERS[prefix]!.includes(file) ? [`${file} 借用了 ${name}`] : [];
  });
}

function i18nViolations(file: string, source: string): string[] {
  return creatorKeysOf(source).flatMap((key) => {
    if (SHARED_SECTIONS.some((shared) => key === shared || key.startsWith(`${shared}.`))) return [];
    const section = longestPrefix(key, I18N_SECTION_OWNERS, ".");
    return section && !I18N_SECTION_OWNERS[section]!.includes(file) ? [`${file} 借用了 ${key}`] : [];
  });
}

/** 往全局状态栏写结果、或用裸 `luna-empty` 当加载中。 */
function feedbackViolation(source: string): boolean {
  return /\bonStatus\b|className="luna-empty"/.test(stripComments(source));
}

const files = Object.entries(SOURCES).map(([path, source]) => [path.replace(/^\.\//, ""), source] as const);

describe("创作空间的应用内统一规范（ADR-165 ⑨）", () => {
  it("扫到了创作空间的源码（守护自身不能空转）", () => {
    expect(files.length).toBeGreaterThan(20);
    expect(files.flatMap(([, source]) => classNamesOf(source)).length).toBeGreaterThan(100);
  });

  it("各页只用自己的类名，共用的走 luna-creator-page__", () => {
    expect(files.flatMap(([file, source]) => classViolations(file, source))).toEqual([]);
  });

  it("反向夹具：概览借发布页的类名会红；注释里提到不算", () => {
    expect(classViolations("ProjectOverview.tsx", '<p className="luna-publish__hint">x</p>')).toEqual([
      "ProjectOverview.tsx 借用了 luna-publish__hint",
    ]);
    expect(classViolations("ProjectOverview.tsx", '/* luna-publish__hint */ <p className="luna-creator-page__hint" />')).toEqual(
      [],
    );
    expect(classViolations("CreatorNotePage.tsx", 'className={`luna-project__body ${x}`}')).toEqual([
      "CreatorNotePage.tsx 借用了 luna-project__body",
    ]);
  });

  it("i18n 不跨节借键", () => {
    expect(files.flatMap(([file, source]) => i18nViolations(file, source))).toEqual([]);
  });

  it("反向夹具：渠道包装借章节的键、灵感库借项目页的键会红；共用词表不红", () => {
    expect(i18nViolations("PackageSection.tsx", 't("creator.publish.chapters.save")')).toEqual([
      "PackageSection.tsx 借用了 creator.publish.chapters.save",
    ]);
    expect(i18nViolations("SwipeLibrary.tsx", 't("workspace:creator.project.crumbLabel")')).toEqual([
      "SwipeLibrary.tsx 借用了 creator.project.crumbLabel",
    ]);
    expect(
      i18nViolations("CreatorViews.tsx", 't(`creator.nextAction.${a}`); t("creator.channel.youtube")'),
    ).toEqual([]);
  });

  it("创作页不往全局状态栏写结果，加载中只有一种样子", () => {
    expect(files.filter(([, source]) => feedbackViolation(source)).map(([file]) => file)).toEqual([]);
    // 反向夹具。
    expect(feedbackViolation("onStatus(vaultErrorText(error))")).toBe(true);
    expect(feedbackViolation('<p className="luna-empty">加载中</p>')).toBe(true);
    expect(feedbackViolation("// 以前是 onStatus，现在就近说\nfail(text)")).toBe(false);
  });
});
