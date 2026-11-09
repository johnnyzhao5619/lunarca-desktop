/**
 * 标注块（callout / admonition）语法模型——**编辑器装饰与阅读模式渲染的单一来源**
 * （PRD-06-011，Obsidian callout 对标）。
 *
 * 语法（Obsidian 兼容，建立在标准引用块之上）：
 * ```
 * > [!warning] 可选标题
 * > 正文
 * ```
 * 类型后的 `-` / `+` 表示默认折叠 / 默认展开（Obsidian 记法，这里只有阅读模式能折叠：
 * 编辑态把正文藏起来等于让人改不到自己写的字）。
 *
 * ## 为什么不做图标
 *
 * Obsidian 的标注块靠"图标 + 配色"辨识。本项目的图标资产是 React 组件（`ui/Icon.tsx`），
 * 而 Live Preview 的装饰层是手写 DOM——要在两个渲染面都出图标，就得把同一批路径数据抄第二份，
 * 那是必然漂移的第二事实源。故改为 **配色 + 类型名标签**：标签取自文档里写的那个词
 * （`[!warning]` → `WARNING`），是文档内容不是界面文案，天然不进 i18n、也不会与正文语言冲突。
 *
 * ## 色调只有五档
 *
 * 设计体系里没有蓝色语义（TDD-11），紫又专供 AI 语义，故十几种 Obsidian 类型收敛到五档
 * 既有语义色。同族别名共用一档不是偷懒：`danger`/`error`/`bug` 本来就是同一种"出事了"。
 */

/** 标注块色调（对应设计 Token 的语义色族）。 */
export type CalloutTone = "note" | "success" | "warning" | "danger" | "quote";

/** 类型词 → 色调。键一律小写；未登记的类型退 `note`（陌生标注仍是标注，不退化成普通引用）。 */
const TONE_BY_TYPE: Record<string, CalloutTone> = {
  note: "note",
  info: "note",
  todo: "note",
  question: "note",
  help: "note",
  faq: "note",
  tip: "success",
  hint: "success",
  important: "success",
  success: "success",
  check: "success",
  done: "success",
  warning: "warning",
  caution: "warning",
  attention: "warning",
  // 提词器录制标记（ADR-162 ⑤ C4c）：「这一句录了几遍」是给剪辑的提醒。
  retake: "warning",
  danger: "danger",
  error: "danger",
  bug: "danger",
  failure: "danger",
  fail: "danger",
  missing: "danger",
  quote: "quote",
  cite: "quote",
  example: "quote",
  abstract: "quote",
  summary: "quote",
  tldr: "quote",
};

export interface CalloutMarker {
  /** 原样的类型词（小写归一，如 `warning`）。 */
  type: string;
  tone: CalloutTone;
  /** 展示用类型标签（大写，取自文档内容而非 i18n）。 */
  label: string;
  /** 默认折叠（`-`）；`+` 与缺省都是展开。 */
  defaultFolded: boolean;
  /** `[!type]±` 标记在传入文本中的结束偏移（其后即标题文本起点）。 */
  markerEnd: number;
  /** 标题文本（可空）。 */
  title: string;
}

/** `[!type]` 必须紧跟引用内容开头，`]` 后可带一个折叠符，再后是可选标题。 */
const MARKER_RE = /^(\s*)\[!([A-Za-z][\w-]*)\]([+-]?)[ \t]?(.*)$/;

/**
 * 解析标注头。传入的是**引用标记之后**的内容（`> ` 已由调用方剥掉）。
 * 不是标注头返回 `null`——普通引用块保持普通引用。
 */
export function parseCalloutMarker(content: string): CalloutMarker | null {
  const match = MARKER_RE.exec(content);
  if (!match) {
    return null;
  }
  const [, indent, rawType, fold, rest] = match;
  const type = rawType.toLowerCase();
  return {
    type,
    tone: TONE_BY_TYPE[type] ?? "note",
    label: rawType.toUpperCase(),
    defaultFolded: fold === "-",
    markerEnd: indent.length + rawType.length + 3 + fold.length,
    title: rest,
  };
}

/** 一行里引用标记（`>`、`> >`…）占据的前缀长度；不是引用行返回 0。 */
export function quotePrefixLength(line: string): number {
  const match = /^(\s*(?:>\s?)+)/.exec(line);
  return match ? match[1].length : 0;
}

/**
 * 只剥**一层**引用标记（`> ` → ``；`> > ` → `> `）。
 *
 * 与 [`quotePrefixLength`] 分工：那个量的是"这一行有多少字符属于引用标记"（编辑器要拿它
 * 定位 `[!type]` 在文档里的绝对位置），这个用于把嵌套引用逐层交还给解析器——一次剥光会
 * 把标注块里的嵌套引用压平成同级正文。
 */
export function stripQuoteLevel(line: string): string {
  return line.replace(/^(\s*)>[ \t]?/, "$1");
}
