/// <reference types="vite/client" />
/**
 * 选中态口径守护（裁决 D5 / TDD-15 §3.3a）。
 *
 * 同一个"选中"，界面上曾有三种说法：活动栏用 `aria-pressed`、笔记树用 `aria-pressed`、
 * 标签树还额外挂了一个**没有任何人消费**的 `data-selected`。三种写法读屏念出来是三种意思。
 *
 * 口径只有一条，按**这个控件在回答什么问题**分：
 * - 「我现在在哪儿 / 哪一条是当前打开的」→ `aria-current`（活动栏、笔记行、标签行）。
 *   读屏念「当前项」，这正是用户要的那句话。
 * - 「这个开关按下去了吗」→ `aria-pressed`（静音、翻译、只看我加的标签、排序方式、
 *   段控的每一格）。它是**两态开关**，与"当前在哪儿"无关。
 *
 * 判据只认一个机器读得出的签名：`aria-pressed` 不得绑定到"**某个选中标识的相等比较**"
 * （`selectedPath === …` / `=== activeTab` 这类）——那正是"当前是哪一条"，不是开关。
 * 段控里的 `mode === "live"` 这种也是相等比较，但它比的是**模式**不是被选中的条目，
 * 故白名单在判据里写死为"比较对象名里含 selected/active 且不是模式名"。
 *
 * 还有一个**反向**的漏法，上面三条一条也拦不住：样式挂在一个**没有任何组件写过**的状态
 * 属性上。笔记树的三态段控就这么坏了一版——CSS 点亮 `.luna-note-tree__mode-btn
 * [data-active="true"]`，组件只写 `aria-selected`，于是"当前是全部还是知识还是资料"在屏
 * 幕上完全看不出来。它不报错、不掉渲染、不进控制台，只是**永远不命中**：肉眼要盯着截图
 * 才发现，单测又都在断言 DOM 属性而不是最终样式。故第四条判据从 CSS 那头查：类名上直接
 * 挂状态属性的选择器，必须有组件真的把这个属性写在带该类名的元素上。
 *
 * 第五条查的是**同一个漏洞的另一头**：组件写对了属性，却没有任何规则点亮它。多选就这么
 * 坏过一版——外观按类名各写一份，接第六条清单（速记）时没人记得补，于是 Cmd 加选确实改了
 * 选择集，屏幕上一动不动。这一条不去逐个清单点名，而是钉住那个让它不可能再漏的安排：
 * 外观的判据必须是选择原语自己贴在行上的属性。
 */

import { describe, expect, it } from "vitest";

const TSX = import.meta.glob(["../{app,features,components}/**/*.tsx", "../../../../packages/*/src/**/*.tsx"], {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const VIEWS = Object.fromEntries(Object.entries(TSX).filter(([p]) => !p.endsWith(".test.tsx")));

// 共享样式包里的 CSS 同样算在内：编辑面与阅读面的规则住在那里（`@lunarca/styles`）。
const STYLES = import.meta.glob(["../**/*.css", "../../../../packages/*/src/**/*.css"], {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/** 表达"选中 / 按下 / 当前"的状态属性——CSS 与组件必须说同一个词，否则规则永不命中。 */
const STATE_ATTRS = [
  "data-active",
  "data-selected",
  "data-current",
  "aria-selected",
  "aria-pressed",
  "aria-current",
] as const;

const STATE_ATTR_ALT = STATE_ATTRS.join("|");

/** 类名边界：`luna-icon-btn` 不该被 `luna-icon-btn--hud` 顶掉。 */
function classNamePattern(className: string): RegExp {
  // 类名来自 CSS 选择器里的 `[\w-]+` 捕获，不含正则元字符，无需转义。
  return new RegExp(`(?<![\\w-])${className}(?![\\w-])`);
}

/** `aria-pressed` 被用来表达"这是当前选中的那一条"的地方。 */
export function pressedUsedForCurrentItem(source: string): string[] {
  return [...source.matchAll(/aria-pressed=\{([^{}]*)\}/g)]
    .map((match) => match[1].trim())
    // 相等比较的**两边都要看**：`item === activeTab` 把标识写在右边，只查左边时
    // 这一处会静默漏过——反向验证正是这么发现的。
    .filter((expr) =>
      /(\b(selected\w*|activeTab|activeItem|current\w*)\s*===|===\s*(selected\w*|activeTab|activeItem|current\w*)\b)/.test(
        expr,
      ),
    );
}

/**
 * `aria-current` 绑**裸布尔比较**的地方：React 对 aria-* 不做布尔省略，
 * `aria-current={false}` 会真的渲染出 `aria-current="false"`——而样式全部按**存在性**
 * 匹配 `[aria-current]`，于是每一行都被点亮成选中态（验收复审当场抓到，七处全中）。
 * 必须写 `expr || undefined`（或产出 "page"/undefined 的三元）。
 */
export function booleanAriaCurrent(source: string): string[] {
  return [...source.matchAll(/aria-current=\{([^{}]*)\}/g)]
    .map((match) => match[1].trim())
    .filter((expr) => expr.includes("===") && !expr.includes("undefined"));
}

/** 无人消费的选中态影子属性（CSS 与 JS 都不读它，只是看着像在表达什么）。 */
export function orphanSelectionAttributes(source: string): string[] {
  return [...source.matchAll(/data-selected=\{[^{}]*\}/g)].map((match) => match[0]);
}

/** CSS 注释里抄的选择器不是规则，不进判据。 */
function stripCssComments(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, "");
}

/**
 * CSS 里**直接挂在类名上**的状态钩子（`.cls[data-active="true"]`）。
 * `.luna-x__tabs button[aria-selected="true"]` 这种载体是标签名不是类名——机器认不出它对应
 * 哪个组件元素，不入判据（宁可放过）。
 */
export function cssStateHooks(source: string): Array<{ className: string; attribute: string }> {
  const pattern = new RegExp(`\\.([a-zA-Z][\\w-]*)\\[(${STATE_ATTR_ALT})`, "g");
  return [...source.matchAll(pattern)].map((match) => ({
    className: match[1],
    attribute: match[2],
  }));
}

/** JSX 开标签。花括号里的 `>` 不算收尾——`onClick={() => …}` 每个标签里都有一个。 */
export function jsxOpeningTags(source: string): string[] {
  return [...source.matchAll(/<[a-zA-Z][^\s/>]*/g)].map((match) => {
    const start = match.index ?? 0;
    let depth = 0;
    let end = start;
    while (end < source.length) {
      const char = source[end];
      if (char === "{") {
        depth += 1;
      } else if (char === "}") {
        depth -= 1;
      } else if (char === ">" && depth === 0) {
        break;
      }
      end += 1;
    }
    return source.slice(start, end);
  });
}

/**
 * 带某个类名的元素。类名字面量写在开标签里是常态；`IconButton` 这类原语把类名拼在变量里
 * （`classes.push("luna-icon-btn")`），标签上只剩 `className={…}`——那就退化成"本文件里所有
 * 动态 className 的标签"，宁可放过也不误伤。
 */
export function classCarrierTags(
  source: string,
  className: string,
  tags: string[] = jsxOpeningTags(source),
): string[] {
  const pattern = classNamePattern(className);
  const direct = tags.filter((tag) => pattern.test(tag));
  if (direct.length > 0) {
    return direct;
  }
  return pattern.test(source) ? tags.filter((tag) => tag.includes("className={")) : [];
}

/** 元素自己写了这个属性，或它展开了外来 props——原语的按下态由调用方给。 */
export function tagWritesState(tag: string, attribute: string): boolean {
  return new RegExp(`${attribute}\\s*=`).test(tag) || tag.includes("{...");
}

/**
 * 永不命中的选中态样式：CSS 点亮某个属性，却没有任何组件把它写在带该类名的元素上。
 * 这类 bug 只在屏幕上可见——DOM 属性断言全绿、控制台干净、样式规则语法合法，就是不亮。
 */
export function orphanCssStateHooks(
  styles: Record<string, string>,
  views: Record<string, string>,
): string[] {
  const scanned = Object.values(views).map((source) => ({
    source,
    tags: jsxOpeningTags(source),
  }));
  const verdicts = new Map<string, boolean>();
  const offenders: string[] = [];

  for (const [path, css] of Object.entries(styles)) {
    for (const { className, attribute } of cssStateHooks(stripCssComments(css))) {
      const hook = `.${className}[${attribute}]`;
      let written = verdicts.get(hook);
      if (written === undefined) {
        written = scanned.some(({ source, tags }) =>
          classCarrierTags(source, className, tags).some((tag) => tagWritesState(tag, attribute)),
        );
        verdicts.set(hook, written);
      }
      if (!written && !offenders.some((entry) => entry.endsWith(hook))) {
        offenders.push(`${path}: ${hook}`);
      }
    }
  }
  return offenders;
}

describe("选中态口径", () => {
  it("扫描面非空（glob 失效时本用例先红）", () => {
    expect(Object.keys(VIEWS).length).toBeGreaterThan(50);
    expect(Object.keys(VIEWS).some((path) => path.includes("/packages/editor-core/src/")), "共享渲染包不在扫描面里").toBe(true);
    expect(Object.keys(STYLES).length).toBeGreaterThan(40);
  });

  it("「当前是哪一条」用 aria-current，不用 aria-pressed", () => {
    const offenders = Object.entries(VIEWS).flatMap(([path, source]) =>
      pressedUsedForCurrentItem(source).map((expr) => `${path}: aria-pressed={${expr}}`),
    );
    expect(
      offenders,
      `导航/清单里的"当前项"请用 aria-current；aria-pressed 只留给两态开关（D5）：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("aria-current 不渲染出 \"false\"（样式按存在性匹配）", () => {
    const offenders = Object.entries(VIEWS).flatMap(([path, source]) =>
      booleanAriaCurrent(source).map((expr) => `${path}: aria-current={${expr}}`),
    );
    expect(
      offenders,
      `React 会把 aria-current={false} 渲染成 "false"，存在性选择器照样命中；写 expr || undefined：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("没有无人消费的 data-selected 影子属性", () => {
    const offenders = Object.entries(VIEWS).flatMap(([path, source]) =>
      orphanSelectionAttributes(source).map((attr) => `${path}: ${attr}`),
    );
    expect(
      offenders,
      `选中态由 aria-current 表达，一处就够；data-selected 没有任何消费者：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("CSS 的选中态样式不会永不命中（属性必须真有组件写）", () => {
    const offenders = orphanCssStateHooks(STYLES, VIEWS);
    expect(
      offenders,
      `样式挂在没人写的属性上＝这条规则永远不亮，且只有肉眼看得出来。改成组件真的写的那个属性（别为了对上 CSS 反过来补一份影子属性）：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("多选的选中态外观只有一处，且判据是原语自己贴的属性", () => {
    // 反向的漏法（真机 2026-09-09）：组件写对了 `aria-selected`，**没有任何规则点亮它**。
    // 速记接上共享行选之后，Cmd 加选确实改了选择集，屏幕上却一动不动——用户看到的就是
    // 「多选做不出来」。根因是选中态外观当初按类名各写一份，接第六条清单时没人记得补。
    //
    // 收法是让它不可能再漏：判据改成 `use-note-selection.ts` 自己贴在行上的
    // `data-row-path`，于是任何接上原语的清单都自动有外观。这条用例钉住这个安排。
    const shared = Object.entries(STYLES).filter(([, css]) =>
      stripCssComments(css).includes("[data-row-path]"),
    );
    expect(shared.map(([path]) => path)).toEqual(["../features/workspace/workspace.css"]);

    // 各清单不许再按自己的类名补一份——那正是会漏掉下一条清单的写法。
    const ROW_CLASSES = [
      "luna-note-tree__item-btn",
      "luna-filtered-list__item-btn",
      "luna-inbox__row-open",
      "luna-chat-history__item",
    ];
    const offenders = Object.entries(STYLES).flatMap(([path, css]) =>
      cssStateHooks(stripCssComments(css))
        .filter(({ className, attribute }) =>
          ROW_CLASSES.includes(className) && attribute === "aria-selected",
        )
        .map(({ className }) => `${path}: .${className}[aria-selected]`),
    );
    expect(
      offenders,
      `多选外观已收成 [data-row-path][aria-selected="true"] 一处；按类名再写一份，下一条清单还会漏：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("反向验证：两条判据各自抓得住违规、放得过合规", () => {
    expect(pressedUsedForCurrentItem("aria-pressed={selectedPath === item.path}")).toHaveLength(1);
    expect(pressedUsedForCurrentItem("aria-pressed={item === activeTab}")).toHaveLength(1);
    // 两态开关：不是"哪一条被选中"。
    expect(pressedUsedForCurrentItem("aria-pressed={microphoneMuted}")).toEqual([]);
    // 段控比的是模式，不是被选中的条目。
    expect(pressedUsedForCurrentItem('aria-pressed={mode === "live"}')).toEqual([]);
    expect(pressedUsedForCurrentItem('aria-pressed={sortMode === "mtime"}')).toEqual([]);

    expect(booleanAriaCurrent("aria-current={a === b}")).toHaveLength(1);
    expect(booleanAriaCurrent("aria-current={(a === b) || undefined}")).toEqual([]);
    expect(booleanAriaCurrent('aria-current={a === b ? "page" : undefined}')).toEqual([]);
    expect(booleanAriaCurrent("aria-current={isCurrent}")).toEqual([]);

    expect(orphanSelectionAttributes("data-selected={a === b}")).toHaveLength(1);
    expect(orphanSelectionAttributes("aria-current={a === b}")).toEqual([]);
  });

  it("反向验证：永不命中的选中态样式抓得住，正常写法放得过", () => {
    // 只取直接挂在类名上的属性；载体是标签名的那种判不了，不入判据。
    expect(cssStateHooks('.luna-x[data-active="true"] {}')).toEqual([
      { className: "luna-x", attribute: "data-active" },
    ]);
    expect(cssStateHooks('.luna-x button[aria-selected="true"] {}')).toEqual([]);
    expect(cssStateHooks(".luna-x:hover {}")).toEqual([]);

    // 开标签扫描：花括号里的 `>` 不收尾。
    expect(jsxOpeningTags("<button onClick={() => go()} data-active=\"true\">x</button>")[0]).toBe(
      '<button onClick={() => go()} data-active="true"',
    );

    // 类名边界：`--hud` 修饰类不算 `luna-icon-btn` 的载体。
    expect(classCarrierTags('<i className="luna-icon-btn--hud" />', "luna-icon-btn")).toEqual([]);

    expect(tagWritesState('<b className="luna-x" aria-selected={on}', "aria-selected")).toBe(true);
    expect(tagWritesState('<b className="luna-x" aria-selected={on}', "data-active")).toBe(false);
    // 原语把状态交给调用方：展开了 props 就算有人写。
    expect(tagWritesState("<button {...rest} className={classes.join(\" \")}", "aria-pressed")).toBe(
      true,
    );

    // 笔记树段控当初的原形：CSS 认 data-active，组件只写 aria-selected。
    const brokenCss = { "a.css": '.luna-seg[data-active="true"] { color: red; }' };
    const view = { "A.tsx": '<button className="luna-seg" aria-selected={mode === m} />' };
    expect(orphanCssStateHooks(brokenCss, view)).toEqual(["a.css: .luna-seg[data-active]"]);
    expect(
      orphanCssStateHooks({ "a.css": '.luna-seg[aria-selected="true"] { color: red; }' }, view),
    ).toEqual([]);
    // 注释里抄的选择器不是规则。
    expect(orphanCssStateHooks({ "a.css": '/* .luna-seg[data-active] 见上 */' }, view)).toEqual([]);
  });
});
