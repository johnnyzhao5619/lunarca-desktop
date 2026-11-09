/**
 * 工作区信息架构（ADR-087 / PRD-07-002 /
 * PRD-07-003 / PRD-07-057）——**唯一事实源**。
 *
 * 两层左侧结构：
 * - **活动栏（rail）**：常驻纯图标条，只切换**稳定对象空间**——「我要去哪类内容」。
 * - **主侧栏（sidebar）**：随空间渲染该空间的列表或二级结构；二级只改变该空间的查询投影，
 *   不制造第二份内容。
 *
 * **一份表派生全部**：活动栏默认顺序与贴底系统区、锚点与可配置项、二级归属与默认分区、
 * 有没有侧栏、有没有编辑面、深链落位——全部从 [`WORKSPACE_IA`] 读出（TDD-07 §4「导航结构
 * 与侧栏分发必须由同一份 typed IA 描述派生」）。
 *
 * **业务身份不进这组类型**：它们只表达**界面位置**。`luna.type` / `luna.stage` /
 * 目录 / 索引协议一个字都不受本模块影响（ADR-087 ①）。
 */

import type { CaptureDestinationDto, WorkspaceSpaceDto } from "../../ipc/bindings";

/**
 * 一级对象空间——**权威枚举在 Rust**（`WorkspaceSpaceDto`，ADR-087 ⑦），这里只是别名：
 * 加一个空间是加一个 Rust 枚举值，本表随之编译不过、被迫补齐。
 *
 * 内容空间八个：`notes`（Vault 真实文件树）、`quick_notes`（`type: quick-note` 的捕获流）、
 * `search`、`knowledge`（派生知识矩阵）、`recordings`（`type: recording`）、
 * `transcripts`（`type: transcript`）、`chat`、`creator`（创作空间，归一方应用「创作」，应用
 * 未启用时不在活动栏上，ADR-162 / ADR-165）；系统区 `apps` / `trash` / `settings` 贴底
 * （PRD-07-002 / PRD-07-002b）。
 */
export type WorkspaceSpace = WorkspaceSpaceDto;

/**
 * 二级分区。
 *
 * **只有知识库有分区**（PRD-07-002 钉死；拆分理由归 ADR-088）。其余空间的侧栏内容
 * （文件树 / 捕获胶囊与时间桶 / 查询与结果 / 列表 / 历史会话）是**视图内部结构**，不是
 * 导航分区——把它们建模成 `WorkspaceSection` 就是在造伪分区，用户会以为那里有第二层
 * 可切换的内容归属。
 */
export type WorkspaceSection = "topics" | "tags" | "network";

/** 用户此刻站在哪儿。`section === null` = 该空间没有导航分区。 */
export interface WorkspaceLocation {
  space: WorkspaceSpace;
  section: WorkspaceSection | null;
}

interface SectionSpec {
  id: WorkspaceSection;
  /** i18n key；跨 namespace 用 `ns:key`。 */
  labelKey: string;
  /** 这一分区的主区是多标签/分屏编辑面吗（PRD-07-058）。 */
  editorSurface?: boolean;
}

interface SpaceSpec {
  space: WorkspaceSpace;
  labelKey: string;
  /** `primary` = 活动栏主列表；`system` = 贴底系统维护区（PRD-07-002b）。 */
  slot: "primary" | "system";
  /** 有序二级分区；空数组 = 无导航分区。 */
  sections: readonly SectionSpec[];
  /**
   * 默认分区——**显式给**，不靠「取第一个」：默认与顺序同值只是巧合，下一次调整分区顺序
   * 时靠顺序拿到的答案会静默变错（PRD-07 §3.2）。
   */
  defaultSection: WorkspaceSection | null;
  /** 有主侧栏列表吗；无则侧栏隐藏、主区全宽（PRD-07-057 ③）。 */
  hasSidebar: boolean;
  /** 无分区空间的主区是否为编辑面（笔记与搜索共用同一张桌子，PRD-07-058）。 */
  editorSurface?: boolean;
  /**
   * 主区的**表面类型**（PRD-07-071）：
   * - `document`：一张纸——描边 + 圆角 + 表面底色。给"这一格里有一份东西在被读写"的空间。
   * - `canvas`：透明画布——限宽居中、不画自己的底。给"这一格是一条流/一片图"的空间。
   *
   * **它此前不存在**，于是七个空间各长各的：三个走编辑器的有整张纸、录音与转写是
   * 「有底色没描边」的半张纸、对话与知识库完全透明——而没有任何文档说过哪种是对的
   * （真机 2026-09-08 上报「速记和 AI 对话跟别的页风格不统一」）。缺的不是实现，是判据。
   */
  surface: "document" | "canvas";
  /**
   * 首屏兜底里不画（布局尚未从 Rust 回来时）：创作归一方应用，应用默认未启用（ADR-165 ③）——
   * 先画出来再收回会闪一下。取回之后一律以 Rust 下发的布局为准（未启用的应用整格不下发）。
   */
  hiddenByDefault?: boolean;
  /**
   * 锚点（PRD-07-003 / ADR-087 ⑥）：可排序、**不可隐藏**——笔记是唯一事实源的直接呈现与
   * 默认落点，知识库是派生矩阵的唯一入口。非锚点的内容空间可由用户隐藏；系统区不参与配置。
   */
  anchor: boolean;
}

/**
 * 一级默认顺序、锚点、二级归属、默认分区、是否有侧栏、是否有编辑面——**只在这里定义**。
 *
 * 图标不在表里：它是 JSX，放进这个纯数据模块会把 React 拖进一个被无头测试直接 import
 * 的模块；映射写在 [`WorkspacePrimaryRail`] 的 `Record<WorkspaceSpace, ReactNode>` 里，
 * 由类型穷尽保证不漏（加一个空间当场编译不过），而不是靠第二张需要同步的清单。
 */
export const WORKSPACE_IA = [
  {
    space: "search",
    surface: "document",
    labelKey: "search.title",
    slot: "primary",
    sections: [],
    defaultSection: null,
    hasSidebar: true,
    // 搜索与笔记共用同一片编辑工作面：切换的是左边那一列，不是右边正在写的东西
    //（PRD-07-058）。
    editorSurface: true,
    anchor: false,
  },
  {
    space: "notes",
    surface: "document",
    labelKey: "nav.notes",
    slot: "primary",
    sections: [],
    defaultSection: null,
    hasSidebar: true,
    editorSurface: true,
    anchor: true,
  },
  {
    space: "knowledge",
    surface: "canvas",
    labelKey: "nav.knowledge",
    slot: "primary",
    sections: [
      // 「主题页」= 跨源积累的知识活页目录（PRD-16-015）：回答「关于它我们已经知道什么」。
      // 未选中页时主区呈现库级工作台（PRD-07-024a）。
      { id: "topics", labelKey: "nav.section.topics" },
      // 「标签」= 检索与聚类的底料：回答「哪些笔记提到过它」。两者答的不是同一个问题。
      { id: "tags", labelKey: "nav.section.tags" },
      { id: "network", labelKey: "nav.section.network" },
    ],
    // **显式写死**：它恰好排在第一位，故这一格尤其要写在表里——默认与顺序在这里
    // 同值是巧合，靠「取第一个」拿到的正确答案会在下一次调整分区顺序时静默变错。
    defaultSection: "topics",
    hasSidebar: true,
    anchor: true,
  },
  {
    // 速记 = 语音优先的捕获流（PRD-07-066 / PRD-06-058）：侧栏是捕获胶囊 + 时间桶，主区是
    // 选中条目的编辑器或空白捕获画布——不是多标签/分屏那张桌子。
    space: "quick_notes",
    surface: "document",
    labelKey: "nav.quickNotes",
    slot: "primary",
    sections: [],
    defaultSection: null,
    hasSidebar: true,
    anchor: false,
  },
  {
    space: "recordings",
    surface: "document",
    labelKey: "nav.recording",
    slot: "primary",
    sections: [],
    defaultSection: null,
    hasSidebar: true,
    anchor: false,
  },
  {
    space: "transcripts",
    surface: "document",
    labelKey: "nav.transcripts",
    slot: "primary",
    sections: [],
    defaultSection: null,
    hasSidebar: true,
    anchor: false,
  },
  {
    space: "chat",
    surface: "canvas",
    labelKey: "chat:title",
    slot: "primary",
    sections: [],
    defaultSection: null,
    hasSidebar: true,
    anchor: false,
  },
  {
    // 创作空间（ADR-162 / PRD-19）：侧栏是项目与灵感两份清单，主区是视频项目的阶段看板——
    // 一片可横向铺开的画布，不是一张纸。归一方应用「创作」，启用后才出现（ADR-165 ③）。
    space: "creator",
    surface: "canvas",
    labelKey: "nav.creator",
    slot: "primary",
    sections: [],
    defaultSection: null,
    hasSidebar: true,
    hiddenByDefault: true,
    anchor: false,
  },
  {
    // 提词器空间（ADR-176 ① ②）：一页首页——开始提词、最近提词的稿子、新建提词稿与跟读开关；
    // 没有侧栏。归一方应用「提词器」，安装后才出现。
    space: "teleprompter",
    surface: "canvas",
    labelKey: "nav.teleprompter",
    slot: "primary",
    sections: [],
    defaultSection: null,
    hasSidebar: false,
    hiddenByDefault: true,
    anchor: false,
  },
  {
    // 「应用」（ADR-165 ⑤）：一方应用的列表与详情、启用与停用。贴底在回收站之上。
    space: "apps",
    surface: "canvas",
    labelKey: "nav.apps",
    slot: "system",
    sections: [],
    defaultSection: null,
    hasSidebar: false,
    anchor: true,
  },
  {
    space: "trash",
    surface: "canvas",
    labelKey: "nav.trash",
    slot: "system",
    sections: [],
    defaultSection: null,
    hasSidebar: false,
    anchor: true,
  },
  {
    space: "settings",
    surface: "canvas",
    labelKey: "nav.settings",
    slot: "system",
    sections: [],
    defaultSection: null,
    hasSidebar: true,
    anchor: true,
  },
] as const satisfies readonly SpaceSpec[];

/** 活动栏主列表（内容空间，按表内默认顺序）。 */
export const PRIMARY_SPACES: readonly WorkspaceSpace[] = WORKSPACE_IA.filter(
  (spec) => spec.slot === "primary",
).map((spec) => spec.space);

/** 活动栏贴底系统维护区（PRD-07-002b：不参与主导航滚动，也不参与显隐配置）。 */
export const SYSTEM_SPACES: readonly WorkspaceSpace[] = WORKSPACE_IA.filter(
  (spec) => spec.slot === "system",
).map((spec) => spec.space);

function specOf(space: WorkspaceSpace): SpaceSpec {
  // 表按 `WorkspaceSpace` 穷尽，故此处必然命中；找不到只可能是表被改坏了。
  const spec = WORKSPACE_IA.find((entry) => entry.space === space);
  if (!spec) {
    throw new Error(`WORKSPACE_IA 缺少空间：${space}`);
  }
  return spec;
}

/** 该空间的一级名字键。 */
export function spaceLabelKey(space: WorkspaceSpace): string {
  return specOf(space).labelKey;
}

/** 该空间的二级分区（有序）；空数组 = 无导航分区。 */
export function sectionsOf(space: WorkspaceSpace): readonly SectionSpec[] {
  return specOf(space).sections;
}

/** 该分区的名字键。 */
export function sectionLabelKey(space: WorkspaceSpace, section: WorkspaceSection): string {
  const spec = specOf(space).sections.find((entry) => entry.id === section);
  if (!spec) {
    throw new Error(`空间 ${space} 没有分区 ${section}`);
  }
  return spec.labelKey;
}

/** 该空间的默认分区（无分区空间为 `null`）。 */
export function defaultSectionOf(space: WorkspaceSpace): WorkspaceSection | null {
  return specOf(space).defaultSection;
}

/**
 * 主侧栏列表的分发键：有分区的空间是 `<space>/<section>`，无分区的就是 `<space>`。
 *
 * **由 [`WORKSPACE_IA`] 推导，不是手抄的联合**：`Record<SidebarKey, …>` 因此对
 * 每一格都要求一个渲染器——新增空间或分区时调用方**当场编译不过**，而不是切过去才发现
 * 侧栏空着。
 */
type SidebarSpaceSpec = Extract<(typeof WORKSPACE_IA)[number], { hasSidebar: true }>;

export type SidebarKey = {
  [S in SidebarSpaceSpec as S["space"]]: S["sections"] extends readonly []
    ? S["space"]
    : `${S["space"]}/${S["sections"][number]["id"]}`;
}[SidebarSpaceSpec["space"]];

/**
 * 位置 → 侧栏分发键。
 *
 * 断言是安全的：`location` 只能由 [`locationFor`] 产出，而它保证 `section` 要么属于该
 * 空间、要么是该空间的默认分区——即 `space/section` 这一对必然在 [`SidebarKey`] 里。
 */
export function sidebarKeyOf(location: WorkspaceLocation): SidebarKey {
  return (
    location.section ? `${location.space}/${location.section}` : location.space
  ) as SidebarKey;
}

/**
 * **目的地键**：`<space>/<section>`，无分区的空间就是 `<space>`。
 *
 * **它比 [`SidebarKey`] 多一格**——回收站没有侧栏，但它照样是一个用户会去的地方。
 * 凡是问"用户在哪一格"的消费面（如工作区观测）都该用这个键；用侧栏分发键去回答那个
 * 问题是范畴错误，而 `as` 一下就能让它编译通过，然后在运行时送出一个谁都不认识的值。
 *
 * 同样由 [`WORKSPACE_IA`] 推导：`Record<DestinationKey, …>` 对每一格都要求一个取值，
 * 新增空间或分区时调用方当场编译不过。
 */
type AnySpaceSpec = (typeof WORKSPACE_IA)[number];

export type DestinationKey = {
  [S in AnySpaceSpec as S["space"]]: S["sections"] extends readonly []
    ? S["space"]
    : `${S["space"]}/${S["sections"][number]["id"]}`;
}[AnySpaceSpec["space"]];

/** 位置 → 目的地键（断言安全的理由同 [`sidebarKeyOf`]）。 */
export function destinationKeyOf(location: WorkspaceLocation): DestinationKey {
  return (
    location.section ? `${location.space}/${location.section}` : location.space
  ) as DestinationKey;
}

/** 该目的地是否拥有主侧栏列表（无则侧栏自动隐藏、主区全宽）。 */
export function hasSidebar(space: WorkspaceSpace): boolean {
  return specOf(space).hasSidebar;
}

/**
 * 锚点：可排序、不可隐藏（PRD-07-003）。笔记与知识库是内容锚点；回收站与设置固定贴底、
 * 不参与配置，故同样按锚点回答。
 */
/**
 * 这一空间的主区是纸还是画布（PRD-07-071）。
 *
 * 判据只来自注册表——不看类名、不看"它长得像什么"。加一个空间时 `SpaceSpec` 强制要求
 * 声明它，忘不掉。
 */
export function surfaceOf(space: WorkspaceSpace): "document" | "canvas" {
  return specOf(space).surface;
}

export function isAnchor(space: WorkspaceSpace): boolean {
  return specOf(space).anchor;
}

/** 这一空间是否默认隐藏（布局兜底用；权威口径在 Rust，见 `hiddenByDefault`）。 */
export function isHiddenByDefault(space: WorkspaceSpace): boolean {
  return specOf(space).hiddenByDefault === true;
}

/**
 * 活动栏分隔线落在哪几项**之前**（PRD-07-003「区隔」）。
 *
 * 顺序可由用户自定义，故区隔不能靠「锚点固定在顶部」实现：按当前顺序扫一遍，锚点连续段
 * 与非锚点连续段的边界处各插一根线。默认顺序下是「搜索 | 笔记 知识库 | 速记 录音 转写 AI 对话」
 * 两根；用户调整顺序后随之重算。收成纯函数是为了让这条判据可无头测试。
 *
 * 锚点判据由调用方给：活动栏用 Rust 下发布局里的 `locked`（运行时的权威口径），测试与
 * 兜底用本表的 [`isAnchor`]。
 */
export function railDividerIndices<T>(
  items: readonly T[],
  anchorOf: (item: T) => boolean,
): ReadonlySet<number> {
  const indices = new Set<number>();
  for (let index = 1; index < items.length; index += 1) {
    if (anchorOf(items[index]) !== anchorOf(items[index - 1])) {
      indices.add(index);
    }
  }
  return indices;
}

/**
 * 归一到一个**合法** location：分区不属于该空间（或该空间无分区）时落回默认分区。
 *
 * 深链、活动栏点击、重启恢复全部经这一处——判定散开就会出现"某条路径带着别的空间的
 * 分区"这种只在特定跳转顺序下才复现的状态。
 */
export function locationFor(
  space: WorkspaceSpace,
  section?: WorkspaceSection | null,
): WorkspaceLocation {
  const spec = specOf(space);
  const valid = section && spec.sections.some((entry) => entry.id === section) ? section : null;
  return { space, section: valid ?? spec.defaultSection };
}

/** 两个位置是不是同一处（活动栏选中态与二级选中态的判据）。 */
export function sameLocation(a: WorkspaceLocation, b: WorkspaceLocation): boolean {
  return a.space === b.space && a.section === b.section;
}

/** 该位置的主区是否为多标签/分屏编辑工作面（PRD-07-058）。 */
export function hasEditorSurface(location: WorkspaceLocation): boolean {
  const spec = specOf(location.space);
  if (location.section) {
    return spec.sections.find((entry) => entry.id === location.section)?.editorSurface === true;
  }
  return spec.editorSurface === true;
}

/**
 * 空白编辑画布**长在哪个空间里**（PRD-06-058 B′）。
 *
 * **这是"事实"，不是"判定"**：本函数只回答导航问题（这块画布属于哪一格），而
 * 「记录该存成什么类型、落哪个目录、按哪套惯例命名」全部在 Rust 的
 * `destination_note_plan` 一处（ADR-003 前端零业务）。前端把这个答案原样交给
 * `create_note_from_text`，绝不据此挑命令、挑类型、或写下任何成熟度取值。
 *
 * 「速记」空间里最大最显眼的那块输入区若落盘成知识笔记，与所在空间语义正好相反，而它
 * 和侧栏捕获胶囊长得一模一样——用户只有事后看文件树才发现记错了地方。
 */
export function canvasDestination(location: WorkspaceLocation): CaptureDestinationDto {
  return location.space === "quick_notes" ? "drafts" : "notes";
}

/**
 * 全部落盘目的地——**由 Rust 的 `CaptureDestinationDto` 编译期穷尽**。
 *
 * 写成 `Record<CaptureDestinationDto, true>` 而不是一个手抄的数组：Rust 侧加一个目的地
 * 时这里**编译不过**，于是画布占位/空态/面包屑三处文案的守护会立刻要求补齐它——否则
 * 新目的地会安静地渲染出一整块裸 key，而那正是本仓被咬过三次的漏法。
 */
const CANVAS_DESTINATION_SET: Record<CaptureDestinationDto, true> = {
  notes: true,
  drafts: true,
};

export const CANVAS_DESTINATIONS = Object.keys(
  CANVAS_DESTINATION_SET,
) as readonly CaptureDestinationDto[];

/**
 * 一枚导航项角标（PRD-07-066 速记待处理数）。
 *
 * `numeric` 决定形态：`false`/省略 = 圆点（「有没有」），`true` = 数字（「还剩几个」）。
 */
export interface NavBadge {
  count: number;
  labelKey: string;
  numeric?: boolean;
}

/**
 * 该角标要不要渲染——**只在 > 0 时**。
 *
 * 收成一个纯函数是为了让这条判据可无头测试：一个常驻的 `0` 是在提醒用户一件不存在的
 * 事，而这种"多渲染了一点点"的错在人工检查里几乎不会被当成 bug 报出来。
 */
export function navBadgeToRender(badge: NavBadge | undefined): NavBadge | null {
  return badge && badge.count > 0 ? badge : null;
}

/**
 * 主侧栏拖拽宽度合法区间（px）。前端拖拽 clamp 仅为交互手感，加载端以 Rust
 * `SIDEBAR_WIDTH_MIN/MAX` 为**权威口径**（同值，见 `src-tauri/src/ipc/workspace.rs`）。
 */
export const SIDEBAR_WIDTH_MIN = 180;
export const SIDEBAR_WIDTH_MAX = 600;
