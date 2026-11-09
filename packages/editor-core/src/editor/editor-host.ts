/**
 * 编辑器的宿主能力端口（TDD-17 §2，PRD-21-020）：CodeMirror 6 扩展只说「要做什么」，怎么做归宿主——
 * 桌面经 IPC 问 Rust，网页经 Worker 问 WASM。需要它的扩展工厂一律把它当**必填的第一个参数**：
 * 漏传是类型错误，不是运行时才发现的空操作。
 *
 * 只收「宿主级」的能力：同一个宿主里所有编辑器共用一份（桌面是一个模块级常量）。与某一个编辑器
 * 实例相关的事——当前笔记路径、点开链接之后去哪、界面文案——仍是各扩展自己的参数，由组装编辑器的
 * 那一层按次给。
 *
 * 渲染与库级能力不另起一套：直接复用阅读面的两个端口（`MarkdownHost` / `ReadingVault`），编辑面与
 * 阅读面在同一个宿主上拿到的是同一份实现，两面不会对「这条外链能不能开」「这个嵌入指向哪」各说各话。
 */

import type { ChangeEvent, ComponentType, ReactNode } from "react";
import type { MarkdownHost } from "../host";
import type { ReadingVault } from "../reading/reading-vault";

/** `[[` 补全的一条候选（与桌面 `WikilinkCandidateDto` 同形；排序、消歧与写名字还是写路径都由宿主定）。 */
export interface WikiLinkCandidate {
  /** 笔记标题（候选列表里显示的名字）。 */
  title: string;
  /** 所在文件夹（候选旁注）。 */
  folder: string;
  /** 选中后写进 `[[ ]]` 的目标（同名笔记时是路径）。 */
  insert: string;
  /** 按 frontmatter 别名命中时的那个别名；按真名命中为 `null`。 */
  alias: string | null;
}

/** `#` 补全的一条候选：库里已有的一个标签与用到它的笔记数。 */
export interface TagCount {
  tag: string;
  noteCount: number;
}

/**
 * 属性面板（frontmatter 展开态，ADR-139）从编辑器拿到的入参。面板本身在包里（`PropertiesPanel`），属性
 * 怎么解析、改一项怎么写回原文归宿主的规则端口（`EditorHost.properties`）；编辑器只给原文，并把面板算出的
 * 那一次替换当成一次编辑应用。
 */
export interface PropertiesPanelProps {
  /** 当前 frontmatter 原文（从文首到闭合 `---`）：变了就重读各行。 */
  source: string;
  /** 提交编辑那一刻的 frontmatter 原文（没有了返回 `null`）。 */
  currentSource: () => string | null;
  /** 把一次替换作为编辑器里的一次变更应用（下标是 UTF-16，与编辑器同口径）。 */
  applyChange: (change: { from: number; to: number; insert: string }) => void;
  /** 「在源码中编辑」：切到源码模式并把光标放到这一行（1 起）。 */
  onEditSource: (line: number) => void;
  /** 把焦点还给正文。 */
  focusEditor: () => void;
}

/** 属性值（按写法推断的类型；与 Rust `luna_sync_core::frontmatter_edit::PropertyValue` 的线上形状同形）。 */
export type PropertyValue =
  | { kind: "text"; value: string }
  | { kind: "number"; value: string }
  | { kind: "checkbox"; value: boolean }
  | { kind: "date"; value: string }
  | { kind: "date_time"; value: string }
  | { kind: "list"; value: string[] };

export type PropertyKind = PropertyValue["kind"];

/** 属性为什么只读 / 编辑为什么没做（Rust `EditSkip` 的稳定码）。 */
export type PropertySkip =
  | "unsupported_form"
  | "inline_comment"
  | "multi_line"
  | "nested"
  | "duplicate_key"
  | "unclosed"
  | "key_exists"
  | "invalid_key"
  | "not_found"
  | "invalid_value";

/** 面板上的一行：能改时 `value` 有值；只读时 `locked` 说原因，界面照 `raw` 原文显示。 */
export interface PropertyRow {
  key: string;
  value: PropertyValue | null;
  locked: PropertySkip | null;
  raw: string;
  /** 键所在的行（1 起，按整个文件计）：「在源码中编辑」跳到这里。 */
  line: number;
}

/** 面板的一次编辑；`retype` 的 `today` / `now` 是界面按本地时区给的（现值认不出日期时用）。 */
export type PropertyEdit =
  | { op: "set"; key: string; value: PropertyValue }
  | { op: "add"; key: string; value: PropertyValue }
  | { op: "remove"; key: string }
  | { op: "rename"; from: string; to: string }
  | { op: "retype"; key: string; kind: PropertyKind; today: string; now: string };

/** 一次最小替换（UTF-16 下标，与编辑器位置口径一致）；`from == to` 且 `insert` 为空 = 没改。 */
export interface TextChange {
  from: number;
  to: number;
  insert: string;
}

export type PropertyResult<T> = { ok: true; value: T } | { ok: false; skip: PropertySkip };

/**
 * 属性面板的规则（ADR-139、ADR-003）：读各行、算一次编辑的替换。规则只在 Rust 的
 * `luna_sync_core::frontmatter_edit` 里——桌面经 IPC、网页经 Worker 里的 WASM 调同一份，改出来的字节一致。
 */
export interface PropertyRules {
  read(source: string): Promise<PropertyResult<PropertyRow[]>>;
  edit(source: string, edit: PropertyEdit): Promise<PropertyResult<TextChange>>;
}

/** 菜单里的一项（菜单怎么开、怎么定位、键盘怎么走归宿主）。 */
export type EditorMenuEntry =
  | {
      kind: "action";
      id: string;
      label: string;
      checked?: boolean;
      /** 已格式化的快捷键提示（`formatShortcut`）。 */
      shortcut?: string;
      icon?: ReactNode;
      onSelect: () => void;
    }
  | { kind: "separator"; id: string };

/**
 * 点开即弹菜单的按钮。`icon` 形态画成宿主的图标按钮（名字同时是可访问名与提示）；`text` 形态画成一颗带文字
 * 的按钮（类名与提示由调用方给，可访问名就是按钮里的字）。
 */
export type EditorMenuButtonProps = {
  entries: readonly EditorMenuEntry[];
  /** 菜单本身的无障碍名（缺省用宿主的通用名）。 */
  menuLabel?: string;
} & (
  | { kind: "icon"; label: string; icon: ReactNode }
  | { kind: "text"; title: string; className: string; children: ReactNode }
);

export interface EditorIconButtonProps {
  /** 这颗按钮做什么：同时是可访问名与悬停提示。 */
  label: string;
  icon: ReactNode;
  /** 破坏性动作（删除一项）。 */
  tone?: "danger";
  onClick: () => void;
}

export interface EditorSelectBoxProps {
  ariaLabel: string;
  value: string;
  onChange: (event: ChangeEvent<HTMLSelectElement>) => void;
  /** `<option>` 列表。 */
  children: ReactNode;
}

/**
 * 包里的界面（格式工具条、属性面板）用到的宿主控件原语（TDD-17 §2）：按钮、下拉、菜单与输入框的外观和
 * 行为各宿主各有一套（桌面是全应用唯一的菜单宿主与控件原语，网页是自己的控件），包只说「这里要一个什么」。
 */
export interface EditorUi {
  IconButton: ComponentType<EditorIconButtonProps>;
  SelectBox: ComponentType<EditorSelectBoxProps>;
  MenuButton: ComponentType<EditorMenuButtonProps>;
  /** 嵌在面板里的小号单行输入框的类名。 */
  inputClassName: string;
  /** 字段下方一行错误说明的类名。 */
  fieldErrorClassName: string;
}

/** 库外附件有了 / 没了（CM 侧要的形状：订阅 + 当前代际号；React 侧仍走 `ReadingVault.useAssetGeneration`）。 */
export interface EditorAssetChanges {
  /** 订阅附件变化，返回退订函数。宿主没有这件事就从不回调。 */
  subscribe(listener: () => void): () => void;
  /** 当前附件代际号：变了，图片部件就不复用旧 DOM、重新解析地址。 */
  generation(): number;
}

export interface EditorHost {
  /**
   * 渲染相关的宿主能力，与阅读面同一份：代码块的「复制」按钮（编辑面与阅读面是同一颗）、外链判定与
   * 打开（编辑面点折叠的 `[文本](https://…)`）、公式与图表的画法（嵌入部件里的阅读视图、实时预览的图表）。
   */
  markdown: MarkdownHost;
  /**
   * 库级能力，与阅读面同一份：wiki 目标指不指得到（链接着色）、库内相对链接打开哪一篇、嵌入 `![[…]]`
   * 指向什么、库内图片的地址。
   */
  vault: ReadingVault;
  /** 库外附件变化：实时预览里的图片据此变成断链 / 重新出现（PRD-06-012 ③）。 */
  assets: EditorAssetChanges;
  /** `[[` 补全：这一篇（同名消歧的锚点）里按查询词给候选；失败给空表。 */
  wikiLinkCandidates(notePath: string, query: string): Promise<WikiLinkCandidate[]>;
  /** `#` 补全：全库标签与各自的笔记数；失败给 `null`（不弹补全）。 */
  tagCounts(): Promise<TagCount[] | null>;
  /** frontmatter 展开后属性面板的规则（读各行、算一次编辑的替换）。 */
  properties: PropertyRules;
  /** 包里的界面用到的控件原语（属性面板；格式工具条由宿主直接传同一份）。 */
  ui: EditorUi;
  /**
   * 切换「Tab 键移动焦点」（WCAG 2.1.2 键盘出路）。开关是宿主**会话级的一份**——所有编辑器跟它走、
   * 状态栏读它显示指示；编辑器只把 ⌘⇧M（与 CM 自带的 ⇧⌥M）接到这里。
   */
  toggleTabFocusMode(): void;
}
