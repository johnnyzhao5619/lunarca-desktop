/// <reference types="vite/client" />
/**
 * 控件原语的守护（TDD-15 §3.3a / §3.3c）。
 *
 * `dialog-primitive.test.ts` 管的是「对话框卡片」这一种外壳。这一份管其余五种被反复
 * 重造的控件：**文本按钮 / 图标按钮 / 查询框 / 空态 / 胶囊开关 / 计数徽标**。
 *
 * 每条判据都对应一件真实发生过的事，而不是一条美学偏好：
 *
 * ① **`.luna-btn` 必须带形态修饰符**。基类只定几何与交互、不定颜色，因此裸用时它既没有
 *    底也没有 hover——审查里「裸 `.luna-btn` 没有任何 hover 反馈」的 18 处就是这么来的。
 *    修法不是给基类补一份 hover（那会与各形态的 hover 撞特异性），而是要求每个使用点
 *    显式选一种形态。
 *
 * ② **被收编的旧类名不得复活**。`.luna-capture-empty` / `.luna-search__empty` /
 *    `.luna-chat__empty` / `.luna-graph__toggle` / `.luna-btn--danger-sm` /
 *    `.luna-sidebar-header__action` 等都曾是"某个 feature 的私有实现"，收编后若有人凭
 *    记忆再写一遍，机器必须当场说不——否则原语只是一条建议。
 *
 * ③ **元素兜底选择器不得声明状态属性**（§3.3c）。四处 `<容器> button { … }` 曾把 hover、
 *    disabled、`[aria-pressed]` 写在标签选择器上，特异性恒压类原语，逼出一连串"提特异性
 *    夺回"的补丁。判据收窄到**状态选择器**上：容器内做几何重置是合理的，写状态色不是。
 */

import { describe, expect, it } from "vitest";

const TSX = import.meta.glob(["../{app,features,components}/**/*.tsx", "../../../../packages/*/src/**/*.tsx"], {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

// 共享样式包里的 CSS 同样算在内：编辑面与阅读面的规则住在那里（`@lunarca/styles`）。
const CSS = import.meta.glob(["../{styles,components,features}/**/*.css", "../../../../packages/*/src/**/*.css"], {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/** 逐个 `className="…"` 字面量取出其类列表。 */
export function classLists(source: string): string[][] {
  return [...source.matchAll(/className="([^"{}]+)"/g)].map(([, value]) => value.split(/\s+/));
}

/** 用了 `.luna-btn` 却没选任何形态修饰符。 */
export function bareButtonUsages(source: string): string[] {
  return classLists(source)
    .filter((classes) => classes.includes("luna-btn"))
    .filter((classes) => !classes.some((name) => name.startsWith("luna-btn--")))
    .map((classes) => classes.join(" "));
}

/**
 * 已被收编、不得再出现在任何组件类名里的旧类。
 *
 * 只列**整块类**：`__子元素` 仍可能是某个 feature 自己的东西（如 `.luna-graph-side__cta-badge`
 * 只负责"贴到最右"，形态来自 `.luna-count-badge`）。
 */
export const RETIRED_CLASSES = [
  "luna-btn--danger-sm",
  "luna-capture-empty",
  "luna-search__empty",
  "luna-chat__empty",
  "luna-graph__toggle",
  "luna-sidebar-header__action",
  "luna-search__query",
  "luna-search__input",
  "luna-trash__count",
  "luna-search__note-count",
  "luna-tags-list__item-count",
  "luna-chat-history__new",
  "luna-hud-icon-btn",
  "luna-voice__config-summary",
  "luna-rec-hud__toggle",
  "luna-voice__toggle-btn",
  "luna-rec-hud__bar",
  "luna-proc-progress__bar",
  "luna-settings-field__error",
] as const;

export function retiredClassUsages(source: string): string[] {
  return classLists(source)
    .flat()
    .filter((name) => (RETIRED_CLASSES as readonly string[]).includes(name));
}

/**
 * 曾经用元素兜底给**带原语类的按钮**写状态属性的五个作用域。
 *
 * 判据刻意**只锁这五个**，不写成一条通法。「容器 + 裸 `button`」本身没有错——段控、
 * 上下文面板页签、弹窗翻译窗口里的按钮确实没有自己的类，由容器统一出样是合理且唯一的
 * 定义点。真正的病是**容器内同时住着 `.luna-btn` / `.luna-icon-btn`**：
 * 兜底 (0,1,1) 恒压类原语 (0,1,0)，于是"我明明挂了次要按钮的类"却渲染成另一种东西，
 * 接下来只能靠提特异性夺回（本仓库为此写过三处补丁）。那个条件静态判不出来，硬写成
 * 通法只会得到一个大面积误报的门禁——**一个总在无意义地红的门禁与没有门禁是一回事**。
 * 故此处是**回归锁**：这五个作用域已经清干净，谁也别再往回写。
 */
export const REPAIRED_FALLBACK_SCOPES = [
  ".luna-editor__actions",
  ".luna-rec-hud",
  ".luna-usage__toolbar",
  ".luna-notification-history__actions",
  ".luna-workspace__topbar",
] as const;

export function fallbackStateSelectors(css: string): string[] {
  const offenders: string[] = [];
  for (const match of css.matchAll(/(?:^|\})\s*([^{}@]+?)\s*\{/g)) {
    for (const part of match[1].split(",")) {
      const trimmed = part.trim();
      if (!REPAIRED_FALLBACK_SCOPES.some((scope) => trimmed.startsWith(`${scope} `))) continue;
      // 末段是裸标签（前面有空格 = 后代选择器），且带状态限定。
      if (!/\s(?:button|input|select|textarea|a)(?::|\[)/.test(trimmed)) continue;
      if (!/(:hover|:active|:disabled|\[aria-pressed|\[aria-expanded|\[aria-selected)/.test(trimmed))
        continue;
      // `:not(...)` 形式同样在列——它正是"用优先级对抗"的证据。
      offenders.push(trimmed);
    }
  }
  return offenders;
}

/**
 * 代码里用 `querySelector(".luna-…")` 查到的类名。
 *
 * 这条判据来自本轮的真实事故：查询框收编为原语后，「聚焦搜索框」的快捷键仍在按旧类名
 * `.luna-search__input` 查 DOM——**没有任何断言会红**（类型对、测试全绿、构建成功），
 * 而那个快捷键从此静默失效。类名改名是重构器帮不上忙的地方：它在字符串里。
 */
export function queriedClassNames(source: string): string[] {
  const names: string[] = [];
  for (const match of source.matchAll(/querySelector(?:All)?<[^>]*>?\(\s*"([^"]+)"/g)) {
    for (const token of match[1].matchAll(/\.(luna-[\w-]+)/g)) {
      names.push(token[1]);
    }
  }
  return names;
}

describe("控件原语 · 只有一个定义点", () => {
  it("扫描面非空（glob 失效时本用例先红，而不是让下面的断言凭空通过）", () => {
    expect(Object.keys(TSX).length).toBeGreaterThan(50);
    expect(Object.keys(CSS).length).toBeGreaterThan(10);
  });

  it("每一处 .luna-btn 都显式选了形态（--primary / --secondary / --danger…）", () => {
    const offenders = Object.entries(TSX).flatMap(([path, source]) =>
      bareButtonUsages(source).map((usage) => `${path}: ${usage}`),
    );
    expect(
      offenders,
      `.luna-btn 是抽象基类，没有底也没有 hover；请显式选形态：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("被收编的旧类名没有复活", () => {
    const offenders = Object.entries(TSX).flatMap(([path, source]) =>
      retiredClassUsages(source).map((name) => `${path}: ${name}`),
    );
    expect(
      offenders,
      `这些类已收编进 components/ 原语，请改用对应原语：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("五个已修复的作用域没有再用元素兜底写状态属性（§3.3c 回归锁）", () => {
    const offenders = Object.entries(CSS).flatMap(([path, css]) =>
      fallbackStateSelectors(css).map((selector) => `${path}: ${selector}`),
    );
    expect(
      offenders,
      `状态属性只能由控件自身的类负责（兜底块会恒压类原语）：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("每个被 querySelector 查的 .luna- 类都真的会被渲染出来", () => {
    const rendered = new Set(
      Object.values(TSX)
        .flatMap((source) => classLists(source))
        .flat(),
    );
    const offenders = Object.entries(TSX).flatMap(([path, source]) =>
      queriedClassNames(source)
        .filter((name) => !rendered.has(name))
        .map((name) => `${path}: .${name}`),
    );
    expect(
      offenders,
      `这些类没有任何组件会渲染出来，查询恒返回 null（改名时最容易漏的地方）：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("反向验证：四条判据各自抓得住违规、放得过合规", () => {
    expect(queriedClassNames('querySelector<HTMLInputElement>(".luna-a .luna-b")')).toEqual([
      "luna-a",
      "luna-b",
    ]);
    expect(queriedClassNames('querySelector("#root")')).toEqual([]);

    expect(bareButtonUsages('<button className="luna-btn" />')).toEqual(["luna-btn"]);
    expect(bareButtonUsages('<button className="luna-btn luna-btn--secondary" />')).toEqual([]);
    expect(bareButtonUsages('<span className="luna-btn-like" />')).toEqual([]);

    expect(retiredClassUsages('className="luna-capture-empty"')).toEqual(["luna-capture-empty"]);
    expect(retiredClassUsages('className="luna-empty-state"')).toEqual([]);
    // 子元素类不在列（它可能仍是该 feature 自己的布局）。
    expect(retiredClassUsages('className="luna-graph-side__cta-badge"')).toEqual([]);

    expect(fallbackStateSelectors(".luna-rec-hud button:hover { color: red; }")).toHaveLength(1);
    expect(
      fallbackStateSelectors('.luna-editor__actions button[aria-pressed="true"] { color: red; }'),
    ).toHaveLength(1);
    // 几何重置不在列（容器统一按钮外形是合理的）；作用域外的段控同理放行。
    expect(fallbackStateSelectors(".luna-rec-hud button { border: 0; }")).toEqual([]);
    expect(fallbackStateSelectors(".luna-editor__mode-toggle button:hover { color: red; }")).toEqual(
      [],
    );
  });
});
