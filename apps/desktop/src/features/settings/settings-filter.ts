/**
 * 设置中心的纯派生逻辑（可测、零 IPC）：可见性、大类 / 分组、搜索、「已改动」判定。
 *
 * 前端零业务（ADR-003）：这里**不含任何取值范围/选项集合**——那些随规格从 Rust 注册表
 * 下发（`SettingSpecDto.control`）。本模块只回答「这一屏该显示哪些行、按什么顺序」。
 */

import type { SettingSpecDto } from "../../ipc/bindings";
import {
  SETTINGS_PAGES,
  SETTINGS_PANEL_PAGES,
  type SettingsPageId,
} from "./settings-information-architecture";

/** 已修改状态直接消费 Rust 的比较结果。 */
export function isModified(spec: SettingSpecDto): boolean {
  return spec.modified;
}

/** 由某个面板承载：不渲染成设置行，但**搜索照常命中**并深链过去。 */
export function panelRoute(spec: SettingSpecDto): string | null {
  return spec.surface.kind === "panel" ? spec.surface.route : null;
}

/**
 * 用户可见的设置项：剔除内部状态（应用自管、界面上没有它的位置），并按实验室开关隐藏
 * 未启用能力的快捷键。
 *
 * **面板承载的键留在这里**：它们不渲染成设置行（见 `sectionsOfGroup`），但用户看得见、
 * 改得了，只是入口在面板上——搜索必须命中它们。此前它们与内部键共用一个 `hidden_in_list`
 * 而在这一步就被剔掉，于是搜「上传」「转写」找不到那个隐私开关，用户得到的结论是"没这功能"。
 *
 * 密钥项**保留**：它是隐私边界的一部分（「密钥存哪里」本身就是用户该看见的信息），
 * 由控件层如实呈现为不可编辑状态。
 */
export function visibleSpecs(
  specs: readonly SettingSpecDto[],
  options: { advancedMode: boolean },
): SettingSpecDto[] {
  return specs.filter((spec) => {
    if (spec.surface.kind === "internal") return false;
    if (!spec.available) {
      return false;
    }
    if (!options.advancedMode && spec.visibility === "advanced") {
      return false;
    }
    return true;
  });
}

/** 标准模式搜索仍要诚实告知有多少高级命中，但绝不把它们直接混进结果。 */
export function advancedSearchCount(
  specs: readonly SettingSpecDto[],
  query: string,
  text: SettingSearchText,
): number {
  return searchSpecs(specs, query, text).filter(
    (spec) => spec.available && spec.surface.kind !== "internal" && spec.visibility === "advanced",
  ).length;
}

/**
 * 有内容的页，按信息架构顺序（不为空页留位）。
 *
 * "有内容"= 有设置项**或**是面板型页（`SETTINGS_PANEL_PAGES`：内容由页专属面板承载，
 * 注册表里一个键也没有）。
 */
export function orderedPages(specs: readonly SettingSpecDto[]): SettingsPageId[] {
  const present = new Set(specs.map((spec) => spec.group));
  return SETTINGS_PAGES.filter((page) => present.has(page) || SETTINGS_PANEL_PAGES.includes(page));
}

/** 取某页的可见项（保持注册表内的登记顺序）。 */
export function specsOfPage(
  specs: readonly SettingSpecDto[],
  page: SettingsPageId,
): SettingSpecDto[] {
  return specs.filter((spec) => spec.group === page);
}

/** 页内的一个分节（PRD-09-070 ⑦）：标题文案走 `settings:section.<group>.<id>`。 */
export interface SettingsSection {
  /** 分节 id；`null` = 该组未声明分节（渲染为无标题的单段）。 */
  section: string | null;
  items: SettingSpecDto[];
}

/**
 * 页的分节视图：按注册表声明的 `section` 切段，段序与段内顺序都取注册表顺序
 * （Rust 声明页面归属；同名分节按首次出现顺序合并，避免跨领域归并后重复标题）。
 *
 * 标题渲不渲染归视图层：只有一段时不贴标题——标签的职责是区分，没有可区分对象时
 * 它只是噪声（同 [PRD-06-048 ⑥] 那条裁决；文件树那边的段头本身已随 ADR-087 退役，
 * 但"没有可区分对象就不贴标签"这条判断留了下来）。
 */
export function sectionsOfPage(
  specs: readonly SettingSpecDto[],
  page: SettingsPageId,
): SettingsSection[] {
  const sections: SettingsSection[] = [];
  const bySection = new Map<string | null, SettingsSection>();
  // 只有 `row` 进分节：面板承载的键留在 `specs` 里是为了被搜到，渲染成行会与承载它的
  // 面板重复成两个入口（模型分配那五个下拉正是这么在首屏出现过的）。
  for (const spec of specsOfPage(specs, page).filter((spec) => spec.surface.kind === "row")) {
    const section = spec.section ?? null;
    const existing = bySection.get(section);
    if (existing) {
      existing.items.push(spec);
      continue;
    }
    const next = { section, items: [spec] };
    bySection.set(section, next);
    sections.push(next);
  }
  return sections;
}

/**
 * 各页「已改动」项数（页头「恢复本页…」的确认文案与右键菜单可用性据此决定）。
 *
 * 注册表里的 `group` 不在前端 IA 清单内时**不计入**：那一页在界面上根本不存在
 * （CI 由 `check-settings-registry.py` 拦住这种漂移），这里不为它造一个计数。
 */
export function modifiedCountByPage(
  specs: readonly SettingSpecDto[],
): Partial<Record<SettingsPageId, number>> {
  const counts: Partial<Record<SettingsPageId, number>> = {};
  for (const spec of specs) {
    if (spec.resettable && isModified(spec) && (SETTINGS_PAGES as readonly string[]).includes(spec.group)) {
      const page = spec.group as SettingsPageId;
      counts[page] = (counts[page] ?? 0) + 1;
    }
  }
  return counts;
}

/**
 * 搜索匹配所需的本地化文案取值器（i18n 归调用方，纯函数不碰 t()）。
 *
 * **别名走 i18n 而不是 Rust 注册表**：同义词是**语言事实**，与标签、说明同类——中文的
 * 「热键」与英文的 "hotkey" 不是同一份数据的两种写法，注册表里放一串 i18n key 再由前端
 * 解析只是多一层指路。派生约定与 `desc_i18n_key` 同构（标签 key + `_aliases`），故零新
 * 字段、零样板；中英是否成对由 `check-settings-registry.py` 强制（只译一半时
 * 搜索会在一种语言下悄悄失灵）。
 */
export interface SettingSearchText {
  label: (spec: SettingSpecDto) => string;
  description: (spec: SettingSpecDto) => string;
  /** 该项的同义词，空串 = 没声明。 */
  aliases: (spec: SettingSpecDto) => string;
  /** 该项所在分节的标题，空串 = 该项未归节。 */
  sectionLabel: (spec: SettingSpecDto) => string;
  pageLabel: (page: string) => string;
  /** 该页的同义词，空串 = 没声明。**不含页说明**——说明是散文，见 `searchPages`。 */
  pageAliases: (page: string) => string;
}

/** 大小写无关的「任一字段包含」。 */
function matches(needle: string, fields: readonly string[]): boolean {
  return fields.some((field) => field.toLowerCase().includes(needle));
}

/**
 * 搜索的两种结果**互不重叠**：命中的要么是**一行设置**（`searchSpecs`），要么是
 * **一整页**（`searchPages`）。
 *
 * 判据是"命中的是谁的名字"：行的 key / 标签 / 说明 / 同义词 / 它所在分节的名字 → 行；
 * 页的 id / 页名 / 页同义词 → 页。**页名绝不下发给它的行**——那会让"页名里出现过的词"
 * 把整页拉进结果：`general` 的同义词里有「语言」，搜「语言」于是连信息密度、减少动效、
 * 工作区顺序一起给出来（实测 5 → 12 条）。用户要的是那几行语言设置，外加"语言在通用页"
 * 这一条信息——后者是一张卡片，不是十三行噪声。
 */
export function searchSpecs(
  specs: readonly SettingSpecDto[],
  query: string,
  text: SettingSearchText,
): SettingSpecDto[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [];
  return specs.filter((spec) =>
    matches(needle, [
      spec.key,
      text.label(spec),
      text.description(spec),
      text.aliases(spec),
      text.sectionLabel(spec),
    ]),
  );
}

/**
 * 命中的是**一整页**：给一张「去这一页」的卡片。
 *
 * 两件事各自都需要它：① 面板型页（个人词典 / 账户）注册表零键，遍历 specs 的搜索永远
 * 碰不到——搜「词典」「术语」此前是零结果，而个人词典是十一个一级页之一，用户得到的
 * 结论是"这个功能不存在"（`PRD-09-070` ⑥ 要消除的正是这种失败）；② 按页名搜（「快捷键」
 * 「存储」）时，答案本来就是那一页，而不是把它的十几行铺开。
 */
export function searchPages(
  pages: readonly SettingsPageId[],
  query: string,
  text: Pick<SettingSearchText, "pageLabel" | "pageAliases">,
): SettingsPageId[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [];
  return pages.filter((page) =>
    matches(needle, [page, text.pageLabel(page), text.pageAliases(page)]),
  );
}
