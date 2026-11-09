/// <reference types="vite/client" />
/**
 * 样式归属守护（TDD-15 §2.3 就近放置 / §3.2 Token / §3.5b 容器查询）。
 *
 * 这一份管的是「样式住在哪儿、引用了什么」，与 `css-selector-guard`（同名类只有一个定义点）
 * 和 `control-primitives`（同一种控件只有一个原语）分工不同：
 *
 * ① **一个类前缀的主体规则只住在一个文件里**。判据刻意不去比对"目录名与类名对不对得上"
 *    ——`.luna-rec-hud` 住在 `recording/`、`.luna-url-import` 住在 `transcribe/` 都是对的，
 *    从名字推所有权正是一直在说谎的那件事。真正可验证的事实是：同一个前缀的**主体**
 *    （该前缀作为选择器第一个类出现的那些规则）散落在两个文件里，就一定有一处是寄居。
 *    搬家前的现场：搜索视图三十余个选择器住在
 *    `features/knowledge/`、回收站住在 `features/activity/`、录音 HUD 一半在
 *    `features/voice-input/` 另一半在 `features/caption/`（而它自己的 `recording.css`
 *    根本不存在）、状态栏铃铛住在 `features/settings/`。名字与位置互相说谎的代价不是不好看：
 *    下一个改样式的人按目录找不到它，只能靠全局搜类名——而那正是"再抄一份"的开始。
 *
 * ② **`var()` 引用的 Token 必须真的存在**。现场抓到过一处 `var(--color-bg-elevated, …)`：
 *    那个 Token **在整套 Token 里根本没有**，于是它一直静默走 fallback。一个永远走 fallback
 *    的变量引用比直接写目标值更糟——它让人以为存在一个可调的"浮起表面"色。
 *
 * ③ **主区里的响应式判据是容器不是视口**（§3.5b）。主区宽度随主侧栏拖拽独立于视口变化，
 *    同一个视口下它可以从 1000px 被压到 300px；用视口断点写的"窄了就堆叠"在真正窄的时候
 *    根本不触发。独立窗口（HUD / 弹窗 / 引导全屏）里视口**就是**窗口，那里用媒体查询是对的。
 */

import { describe, expect, it } from "vitest";
import { layeredCmContentBox, layeredCmLinePadding } from "@lunarca/lint-config/css-guards";

const CSS = import.meta.glob(["../{styles,components,features}/**/*.css", "../../../../packages/*/src/**/*.css"], {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/** CM 自带类名的收编条（`@lunarca/styles/codemirror.css`）：`.cm-line` / `.cm-content` 物理内距的唯一负责者。 */
const CODEMIRROR_INTEGRATION = "../../../../packages/styles/src/codemirror.css";

const TOKENS =import.meta.glob("../../../../packages/styles/src/tokens/*.css", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/** 运行时由 JS 注入的 CSS 变量（`cssVariables({...})`，§3.2 唯一许可的 inline 形态）。 */
const CODE = import.meta.glob(["../{app,features,components,hooks,lib}/**/*.{ts,tsx}", "../../../../packages/*/src/**/*.{ts,tsx}"], {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/**
 * 全仓 `.ts/.tsx`——**比上面那一份宽**：含 `ipc/`（生成的 bindings，拼接类名的后缀取值就住
 * 在那里的联合类型里）、`i18n/`、`theme/` 与 `main.tsx`。
 *
 * 宽窄不是随手定的，是**方向**决定的：正向判据（用了却没规则）漏掉一个消费面只是少报
 * 一条；反向判据（有规则却没人用，见 ⑤）漏掉一个消费面就直接变成**一条假阳性**——而假阳性
 * 会教下一个人「这条守护会乱叫」，然后它就废了。
 */
const ALL_CODE = import.meta.glob(["../**/*.{ts,tsx}", "../../../../packages/*/src/**/*.{ts,tsx}"], {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

function stripComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, " ");
}

/**
 * 文件里**主体**规则的类前缀：只认选择器里第一个 `.luna-` 类。
 *
 * 后代覆盖（`.luna-popup .luna-icon-btn`）是使用者对原语的合法定制，归使用者所有，故只看
 * 第一个类；`__element` / `--modifier` 归其基类。
 */
export function prefixesOf(css: string): Map<string, number> {
  const out = new Map<string, number>();
  for (const m of stripComments(css).matchAll(/(?:^|\})\s*([^{}@]+?)\s*\{/g)) {
    for (const part of m[1].split(",")) {
      const first = part.match(/\.(luna-[a-z0-9]+(?:-[a-z0-9]+)*)/);
      if (!first) continue;
      const base = first[1].split("__")[0].split("--")[0];
      out.set(base, (out.get(base) ?? 0) + 1);
    }
  }
  return out;
}

/**
 * TSX/TS 里用到、但**整套样式表里没有任何规则匹配**的 `luna-` 类名。
 *
 * 悬垂类名不是"少了点装饰"，它让一段代码**看起来做了一件事而实际什么也没做**。
 * 现场抓到两个，都在同一页上：
 * - `.luna-visually-hidden` 从未定义 → 模型分配下拉的无障碍标签**没有被隐藏**，
 *   于是每个槽位的标题在屏幕上出现两次（真机 2026-08-08 截图坐实）。
 * - `.luna-settings__notice` 从未定义 → 模型页三处提示（含一条 `role="alert"` 的
 *   「服务端清单里没有这个模型」）渲染成与正文毫无区别的裸段落。
 *
 * 判定只认字面量类名（`className="luna-x"` / `"luna-x"` 常量），不做模板拼接推断——
 * 拼出来的类名本就无法静态判定，硬猜只会制造假阳性。
 */
export function danglingClassNames(
  code: Record<string, string>,
  css: Record<string, string>,
): string[] {
  const declared = new Set<string>();
  for (const sheet of Object.values(css)) {
    for (const m of stripComments(sheet).matchAll(/\.(luna-[A-Za-z0-9_-]+)/g)) {
      declared.add(m[1]);
    }
  }
  // **只认 `className`**。此前扫的是所有字符串字面量，于是把 `id="luna-confirm-title"`
  // 这类**元素 id**（`aria-labelledby` 的落点）当成了悬垂类名——它们本来就不该有 CSS 规则，
  // 报出来是纯噪声，而噪声会让基线失去意义（读的人开始默认"这里的条目不用管"）。
  const used = new Set<string>();
  for (const [path, source] of Object.entries(code)) {
    if (path.includes(".test.")) continue;
    for (const m of source.matchAll(/className=(?:\{)?"([^"\n]*)"/g)) {
      for (const token of m[1].split(/\s+/)) {
        if (/^luna-[A-Za-z0-9_-]+$/.test(token)) used.add(token);
      }
    }
  }
  return [...used].filter((name) => !declared.has(name)).sort();
}

/**
 * 无规则但**合法**的结构性类名（BEM 块根 / 元素）。
 *
 * 判据分两类，不能一刀切：
 * - **修饰符（`--`）无规则 = 真死代码**——修饰符的定义就是"这个变体长得不一样"，
 *   没有规则时它加了等于没加。这一类**不设白名单**，见下方的硬断言。
 * - **块根 / 元素无规则 = 未必是错**。`.luna-search__note` 这类 BEM 块根只提供结构与
 *   查询落点，视觉全在子元素上；强行给它补一条空规则才是噪声。
 *
 * 本清单是后者：逐个看过、确认无视觉意图。它**只准变短，不准变长**——新出现一个就要
 * 先回答"它到底要不要样式"，而不是顺手加进来。
 */
const STRUCTURAL_CLASSES_WITHOUT_RULES = [
  "luna-command-list__actions",
  "luna-desktop-lens-trigger__edit",
  "luna-note-tree__icon-dir",
  "luna-note-tree__icon-file",
  "luna-panel-toggle",
  "luna-playback__btn-label",
  // 录音 HUD 的窗口身份类：外壳全部收编进 `.luna-hud-surface`（ADR-111 ② 批 B）后，
  // 它是五扇窗里唯一没有任何根级布局差异的——类保留作 BEM 块根与将来 tier 规则
  // （ADR-112）的落点，视觉此刻确实为零。
  "luna-rec-hud",
  "luna-search__group",
  "luna-search__note",
  "luna-trash-preview__folder",
];

/** 全部已定义的设计 Token 名。 */
export function definedTokens(): Set<string> {
  const names = new Set<string>();
  for (const css of Object.values(TOKENS)) {
    for (const m of css.matchAll(/(--[a-z0-9-]+)\s*:/g)) {
      names.add(m[1]);
    }
  }
  return names;
}

/** 代码里经 `cssVariables({...})` 注入的变量名（动态值的唯一许可通道）。 */
export function injectedVars(sources: Iterable<string>): Set<string> {
  const names = new Set<string>();
  for (const src of sources) {
    for (const m of src.matchAll(/"(--[a-z0-9-]+)"\s*:/g)) {
      names.add(m[1]);
    }
  }
  return names;
}

/**
 * `var(--x)` 引用里三样都不是的那些：不是设计 Token、不是本文件自己声明的局部变量、
 * 也不是代码注入的动态值——那就只能是**打错的名字**，而它会一直静默走 fallback 或继承。
 */
export function phantomTokenRefs(
  css: string,
  tokens: Set<string>,
  injected: Set<string> = new Set(),
): string[] {
  const local = new Set([...stripComments(css).matchAll(/(--[a-z0-9-]+)\s*:/g)].map((m) => m[1]));
  const refs = new Set([...stripComments(css).matchAll(/var\((--[a-z0-9-]+)/g)].map((m) => m[1]));
  return [...refs].filter(
    (name) => !tokens.has(name) && !local.has(name) && !injected.has(name),
  );
}

/**
 * 主区表面（宽度随侧栏拖拽变化）里用了视口断点的文件。
 *
 * 判据刻意按**文件**给白名单而不是按选择器猜：一个样式表服务的是主区还是独立窗口，是它
 * 自己知道的事实，静态推不出来。白名单里的三处都是真正的独立窗口/全屏层。
 */
export const VIEWPORT_BREAKPOINT_ALLOWED = [
  "caption.css", // 实时字幕：独立 HUD 窗口，视口 = 窗口
  "popup-translate.css", // 弹窗翻译：同上
  "onboarding.css", // 引导全屏层（`.luna-onboarding-viewport`）覆盖整个窗口
  "workspace-responsive.css", // 整个工作区外壳的响应式，量的就是窗口
] as const;

export function viewportBreakpoints(css: string): string[] {
  return [...stripComments(css).matchAll(/@media\s*\([^)]*(?:max|min)-width[^)]*\)/g)].map(
    (m) => m[0],
  );
}


describe("样式归属 · 位置、Token 与响应式判据", () => {
  it("扫描面非空（glob 失效时本用例先红）", () => {
    expect(Object.keys(CSS).length).toBeGreaterThan(15);
    expect(definedTokens().size).toBeGreaterThan(100);
    // 两条 CM 物理内距守护按路径豁免收编条：路径对不上时豁免就落空、守护照样绿着空转。
    expect(CSS[CODEMIRROR_INTEGRATION], "CM 收编条不在扫描面里").toContain(".luna-md-surface .cm-line");
  });

  it("每个类前缀的主体规则只住在一个文件里", () => {
    const homes = new Map<string, Map<string, number>>();
    for (const [path, css] of Object.entries(CSS)) {
      for (const [prefix, n] of prefixesOf(css)) {
        if (!homes.has(prefix)) homes.set(prefix, new Map());
        homes.get(prefix)!.set(path, n);
      }
    }
    const offenders: string[] = [];
    for (const [prefix, files] of homes) {
      // `styles/base.css` 是全局基线层：它的规则形如"这一条声明适用于以下十个消费者"，
      // 一条选择器里列出十个前缀是它的**职责**，不是替谁存放主体。
      const homesOutsideBase = [...files.keys()].filter((f) => !f.endsWith("styles/base.css"));
      if (homesOutsideBase.length <= 1) continue;
      // 只有一个家 → 合规；有多个家时，允许其余文件各自只放**一两条**覆盖（那是使用者
      // 对共享件的合法定制，不是"替它存放主体"）。
      // 工作区外壳按**层次**拆成三个文件（骨架 / 导航 / 响应式）是刻意的信息架构，
      // 三处都在 `styles/layout/`、都叫 `.luna-workspace*`——它没有"寄居"这回事。
      if (prefix === "luna-workspace") continue;
      const substantial = [...files.entries()]
        .filter(([f]) => !f.endsWith("styles/base.css"))
        // CM 集成层是刻意跨 feature 的第三方 DOM 边界：它只覆写供应商类，并以编辑面
        // `.luna-md-surface` 限定作用域；是否越权由下面的 CM 物理属性专项守护负责，不属于主体寄居。
        .filter(([f]) => f !== CODEMIRROR_INTEGRATION)
        .filter(([, n]) => n >= 3);
      if (substantial.length > 1) {
        offenders.push(
          `${prefix}: ${substantial.map(([f, n]) => `${f}(${n})`).join(" / ")}`,
        );
      }
    }
    expect(
      offenders,
      `同一个前缀的主体规则散在多个文件里，必有一处是寄居（TDD-15 §2.3）：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("没有引用不存在的设计 Token（那种引用一直在静默走 fallback）", () => {
    const tokens = definedTokens();
    const injected = injectedVars(Object.values(CODE));
    const offenders = Object.entries(CSS).flatMap(([path, css]) =>
      phantomTokenRefs(css, tokens, injected).map((name) => `${path}: var(${name})`),
    );
    expect(
      offenders,
      `这些 Token 没有任何定义点；要么补进 packages/styles/src/tokens/，要么直接写目标值：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("主区表面用容器查询而不是视口断点（§3.5b）", () => {
    const offenders = Object.entries(CSS)
      .filter(([path]) => !VIEWPORT_BREAKPOINT_ALLOWED.some((name) => path.endsWith(name)))
      .flatMap(([path, css]) => viewportBreakpoints(css).map((q) => `${path}: ${q}`));
    expect(
      offenders,
      `主区宽度随侧栏拖拽独立于视口；判据应是 @container：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });


  it(".cm-line 的内距只走 --cm-line-pad-* 钩子（§3.3d：分层声明会被无层注入静默压制）", () => {
    const offenders = Object.entries(CSS)
      // 收编条自身就是那个单点负责者。
      .filter(([path]) => path !== CODEMIRROR_INTEGRATION)
      .flatMap(([path, css]) => layeredCmLinePadding(css).map((hit) => `${path}: ${hit}`));
    expect(
      offenders,
      `对 .cm-line 写 padding 会被 CM 无层 baseTheme 压过（§3.3d）；请设 --cm-line-pad-* 变量：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it(".cm-content 的内距与居中只走集成层收编条", () => {
    const offenders = Object.entries(CSS)
      .filter(([path]) => path !== CODEMIRROR_INTEGRATION)
      .flatMap(([path, css]) => layeredCmContentBox(css).map((hit) => `${path}: ${hit}`));
    expect(
      offenders,
      `对 .cm-content 写 padding/margin 会被 CM 无层 baseTheme 简写压过；请改 editor Token 与集成层收编条：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("没有新的悬垂类名（用了却没有任何规则匹配 = 那一行代码没有生效）", () => {
    const known = new Set(STRUCTURAL_CLASSES_WITHOUT_RULES);
    const dangling = danglingClassNames(CODE, CSS);

    // 修饰符不设白名单：它没有规则就是没生效，没有"合法的空修饰符"这回事。
    const deadModifiers = dangling.filter((name) => name.includes("--"));
    expect(
      deadModifiers,
      `这些 BEM 修饰符没有任何规则——加了等于没加，删掉它或补上样式：\n${deadModifiers.join("\n")}`,
    ).toEqual([]);

    const fresh = dangling.filter((name) => !known.has(name) && !name.includes("--"));
    expect(
      fresh,
      `发现新的悬垂类名——它们在界面上什么也不做：\n${fresh.join("\n")}`,
    ).toEqual([]);
    // 棘轮反向：清掉存量后要把它从基线里删掉，否则基线会慢慢变成一张过时的免罪表。
    const stale = [...known].filter((name) => !dangling.includes(name));
    expect(stale, `这些已经修好了，请从 STRUCTURAL_CLASSES_WITHOUT_RULES 里删掉：\n${stale.join("\n")}`).toEqual(
      [],
    );
  });

  it("反向验证：三条判据各自抓得住违规、放得过合规", () => {
    expect([...prefixesOf(".luna-a__x, .luna-b {}").keys()].sort()).toEqual(["luna-a", "luna-b"]);
    expect(prefixesOf("/* .luna-ghost */ .luna-a {}").has("luna-ghost")).toBe(false);

    const tokens = new Set(["--color-x"]);
    expect(phantomTokenRefs("a { color: var(--nope); }", tokens)).toEqual(["--nope"]);
    expect(phantomTokenRefs("a { color: var(--color-x); }", tokens)).toEqual([]);
    // 本文件自己声明的局部变量不算幽灵。
    expect(phantomTokenRefs("a { --mine: 1px; border: var(--mine); }", tokens)).toEqual([]);
    // 代码注入的动态值同理（`cssVariables({ "--folder-depth": … })`）。
    expect(injectedVars(['cssVariables({ "--folder-depth": String(d) })'])).toContain(
      "--folder-depth",
    );
    expect(
      phantomTokenRefs("a { padding: var(--folder-depth); }", tokens, new Set(["--folder-depth"])),
    ).toEqual([]);

    expect(viewportBreakpoints("@media (max-width: 900px) { a { color: red } }")).toHaveLength(1);
    expect(viewportBreakpoints("@container playback (max-width: 900px) { a { color: red } }")).toEqual(
      [],
    );
    expect(viewportBreakpoints("@media (prefers-reduced-motion: reduce) { a { color: red } }")).toEqual(
      [],
    );

    expect(layeredCmLinePadding(".x .cm-line.y { padding-inline-start: 4px }")).toHaveLength(1);
    expect(layeredCmLinePadding(".x .cm-line.y { padding-block-end: 4px }")).toHaveLength(1);
    expect(layeredCmLinePadding(".x .cm-line.y { --cm-line-pad-inline-start: 4px }")).toEqual([]);
    expect(layeredCmLinePadding(".x .cm-panel { padding: 4px }")).toEqual([]);
  });
});

/**
 * ④ **字体栈只有一处定义，且带 CJK**（TDD-15 §3.2 Token / §3.3c 单一负责者）。
 *
 * 现场：同一个无衬线栈被写了三遍——`html,body,#root` 一份长栈、`.luna-workspace` 一份
 * **短栈**（少了 `"Helvetica Neue", Arial`）、`kbd` 又一份短栈。三份没有一份是错的，合起来
 * 才是错的：`.luna-workspace` 特异性更高，于是整个工作区实际用的一直是那份短的，
 * 长栈从头到尾没生效过，而**没有任何东西会因此变红**。
 *
 * 同一现场还藏着第二件事：三份**都没有 CJK 字体声明**，中文一路落到系统默认。
 * 这在一个笔记以中文为主的产品里不是细节。
 */
describe("样式归属守护 · 字体栈", () => {
  const ALL_CSS = Object.entries(CSS).map(([path, css]) => [path, stripComments(css)] as const);
  const TOKEN_CSS = Object.values(TOKENS).map(stripComments).join("\n");

  it("扫描面非空（glob 失效时本用例先红）", () => {
    expect(ALL_CSS.length).toBeGreaterThan(20);
    expect(TOKEN_CSS.length).toBeGreaterThan(200);
  });

  it("无衬线栈的字面量只出现在 Token 定义处，一处", () => {
    const hits = ALL_CSS.flatMap(([path, css]) =>
      css
        .split("\n")
        .map((line, i) => ({ line, at: `${path}:${i + 1}` }))
        .filter(({ line }) => /-apple-system|BlinkMacSystemFont|"Segoe UI"/u.test(line))
        .map(({ at }) => at),
    );
    expect(hits, `字体栈字面量应当只在 --font-family-base 的定义处：\n${hits.join("\n")}`)
      .toHaveLength(1);
    expect(hits[0]).toContain("tokens/primitive.css");
    expect([...TOKEN_CSS.matchAll(/^\s*--font-family-base\s*:/gmu)]).toHaveLength(1);
  });

  it("反向：任何规则要用无衬线栈都只能 `var(--font-family-base)`", () => {
    const literal = ALL_CSS.flatMap(([path, css]) =>
      css
        .split("\n")
        .map((line, i) => ({ line, at: `${path}:${i + 1}` }))
        // `font-family:` 的值里既不是 var() 也不是 inherit = 又抄了一份名单。
        // Token 定义行（`--font-family-*`）是名单的**唯一合法归属地**，不在此列。
        .filter(
          ({ line }) =>
            /font-family\s*:/u.test(line) &&
            !/^\s*--/u.test(line) &&
            !/var\(|inherit/u.test(line),
        )
        .map(({ at, line }) => `${at}: ${line.trim()}`),
    );
    expect(literal, `发现写死的字体栈：\n${literal.join("\n")}`).toEqual([]);
  });

  it("栈里有 CJK 字体，且**排在拉丁字体之后**（回退是逐字符的）", () => {
    const stack = /--font-family-base\s*:([^;]*);/u.exec(TOKEN_CSS)?.[1] ?? "";
    expect(stack, "--font-family-base 找不到了").not.toBe("");
    // macOS / Windows / 其余落点各有一个名字，并列在**同一个栈**里——不写平台分支。
    for (const face of ["PingFang SC", "Microsoft YaHei", "Noto Sans CJK SC"]) {
      expect(stack, `CJK 栈缺 ${face}`).toContain(face);
    }
    // 顺序：最后一个拉丁字体在第一个 CJK 字体之前；西文才不会被 CJK 的半角字形接管。
    const lastLatin = Math.max(
      ...["-apple-system", '"Segoe UI"', "Roboto", '"Helvetica Neue"', "Arial"].map((f) =>
        stack.indexOf(f),
      ),
    );
    const firstCjk = stack.indexOf('"PingFang SC"');
    expect(firstCjk).toBeGreaterThan(lastLatin);
    // 通用族永远收尾。
    expect(stack.trimEnd().endsWith("sans-serif")).toBe(true);
  });

  it("等宽栈刻意不带 CJK（裁决；多一份名单就多一处要同步的定义）", () => {
    const mono = /--font-family-mono\s*:([^;]*);/u.exec(TOKEN_CSS)?.[1] ?? "";
    expect(mono, "--font-family-mono 找不到了").not.toBe("");
    for (const face of ["PingFang", "YaHei", "CJK"]) {
      expect(mono, `等宽栈不该出现 ${face}`).not.toContain(face);
    }
    expect(mono.trimEnd().endsWith("monospace")).toBe(true);
  });
});

/**
 * ⑤ **反向：样式表里有规则、全仓却没有任何消费者的 `luna-` 类名**（TDD-15 §3.3a 原语退役
 *    之后的残渣）。
 *
 * 上面那条「悬垂类名」是**正向**的：代码里用了、样式表里没有。它的反面——样式表里有、代码
 * 里没有——此前**没有任何东西在管**：`css-selector-guard` 只问「同名类是不是有两个定义
 * 点」，`control-primitives` 的 `RETIRED_CLASSES` 只扫 `.ts/.tsx`（退役类名残在 CSS 选择器
 * 里它照样绿），本文件 ①②③④ 也全在问别的事。于是死规则只进不出：本轮手工扫出并清掉 15 条
 * （`luna-filtered-list__item-row`、`luna-topic-draft__error`、若干只剩修饰符没剩消费者的
 * `--variant`……），它们在仓里活了很久，`lint:css` 一次都没红过。
 *
 * 死规则的代价不是「多几行 CSS」，是**它们看起来是活的**：下一个人照着 `--badge--warn`
 * 的写法给新状态补一个 `--badge--info`，然后花半天找为什么没生效——因为那一族的消费者早就
 * 没了，而样式表读起来完全像还在服役。这条守护加的是**防复发**：存量清完之后，下一条死规则
 * 在它出生的那次提交里就会红，而不是过一年再被人手工扫出来。
 *
 * 判据的难点全在假阳性。三类都**不用白名单**——一串手写字符串只会变成下一个人消警告的地方，
 * 那条守护当场就废了：
 *
 * 1. **注释里的类名**：必须**先剥注释、再取选择器**。手工排查的第一版直接
 *    `grep '\.luna-[a-z-]*'` 扫 CSS，把 `luna-activity` / `luna-activity-grid` 报成死
 *    规则——它们的规则其实早已删干净，命中的是一段解释「已删除」的注释文本。故这里只认
 *    **选择器前导块**（`{` 之前、且不以 `@` 开头的那一段，见 `selectorPreludes`）；注释、
 *    声明值里的字符串、at-rule 前导一概不算。
 *    消费者一侧同理：`.ts/.tsx` 里**整行 `//` 注释**同样不算消费者（`stripCodeComments`）——
 *    否则一条退役类名只要还被谁在注释里念叨一句，这条守护就永远看不见它，而「先只剩注释
 *    引用、再彻底没人用」恰恰是退役类名的标准死法。行尾 `//` 刻意不剥，理由见该函数。
 *
 * 2. **模板拼接族**（`luna-graph-side__node-dot--${kind}` 5 个、
 *    `luna-notification-host--${channel}` 2 个）：CSS 把各变体写死，TSX 里是拼出来的，按
 *    字面搜必然搜不到。判据**从源码里自己长出来**，不靠人维护：扫代码里紧贴 `${` 的那截
 *    `luna-…-` 前缀（`dynamicClassStems`），再要求「前缀 + 后缀」的**后缀本身是代码里真实
 *    存在的字符串字面量**（`suffixLiterals`）——`node-dot--chat` 活，因为 `"chat"` 在代码
 *    里存在；一个谁也没写过的 `node-dot--zzz` 仍然会红。
 *    ⚠️ 后缀集合必须取**全仓**，不能只取拼接点所在的那个文件：`kind` 的取值住在
 *    `ipc/bindings.ts` 的联合类型里，按同文件取会把 chat / note / recording / transcript
 *    四个**全部误报**（实测如此，这也是选全仓的唯一理由）。代价诚实写在这里：同一族内部
 *    只能证伪到「这个后缀全仓根本不存在」这一步。再往前就要猜拼接的实参，那不是静态能做
 *    的事，硬猜换来的假阳性比它挡住的死规则贵。
 *
 * 3. **类型化组件 API**（`luna-icon-btn--{sm,hud,danger,primary}`：`IconButton.tsx` 的
 *    `density?: "sm" | "hud"` 与 `tone?: "danger" | "primary"` 是**声明过的组件契约**；
 *    当前没有任何调用点传 `tone="danger"`，但那条规则不是死的——删了会破坏公开 API）：
 *    **它不需要第三种办法**。`IconButton` 拼类名用的正是 `` `luna-icon-btn--${density}` ``，
 *    而 `"danger"` / `"primary"` 作为联合类型成员本身就是代码里的字符串字面量——第 2 条的
 *    判据原样覆盖它。这不是巧合：「契约声明在类型里、类名生成在拼接处」本来就是同一件事的
 *    两面，**能被拼出来的恰好就是契约允许的**。真要写一条 CSS 支撑一个契约里没有的变体，
 *    它就该红。
 *
 * 前缀另有**两道下限**，防的是同一件事：一个太短的前缀会把整个命名空间一次性洗白，守护当场
 * 永远绿。`` `luna-${mode}` ``（`theme-family.test.ts` 里就有一条）被正则形状直接挡掉——
 * `luna-` 之后必须先有一段真名字；`` `luna-a-${x}` `` 这种「有名字但只有一个字母」的则由字符
 * 数下限挡掉（`luna-` 之后去掉末尾分隔符至少 6 个字符）。测试文件本就不算消费者，是另一道
 * 独立的保险。
 */

/**
 * 选择器前导块：`{` 之前的那一段，且不以 `@` 开头。
 *
 * 与上面的 `prefixesOf` 用的是两条不同取法，差别是刻意的：`prefixesOf` 只认 `}` 或文件
 * 开头打头的前导，于是 `@media { … }` **每个块里的第一条规则会被漏掉**。正向计数漏一条只是
 * 少数一次；反向判据漏一条，等于放过一条死规则。这里改逐字符扫：`{` 结束一个前导，`}` 与
 * `;` 清空缓冲——于是声明值、注释残留与 at-rule 前导都进不来，嵌套块里的规则一条不少。
 */
/**
 * `.ts/.tsx` 的注释剥除：块注释之外，另剥**整行** `//` 注释。
 *
 * 只剥**整行**是有意的，不是没做完：`//` 也会出现在字符串里（`"https://…"`），把行尾
 * `//` 之后一律当注释剥掉，会连同同一行的真实代码一起吃掉——那制造的是**假阳性**（一条活
 * 规则被报成死规则），而假阳性正是这条守护最贵的失败模式（见 ⑤ 抬头）。整行注释里不可能
 * 有代码，剥它没有这个风险。代价诚实写在这里：行尾注释里的类名仍会被当成消费者。
 */
function stripCodeComments(source: string): string {
  return stripComments(source).replace(/^[ \t]*\/\/.*$/gm, " ");
}

export function selectorPreludes(css: string): string[] {
  const out: string[] = [];
  let buf = "";
  for (const ch of stripComments(css)) {
    if (ch === "{") {
      const prelude = buf.trim();
      if (prelude && !prelude.startsWith("@")) out.push(prelude);
      buf = "";
    } else if (ch === "}" || ch === ";") {
      buf = "";
    } else {
      buf += ch;
    }
  }
  return out;
}

/** 一个样式表里真正**被选择器用到**的 `luna-` 类名（分组选择器的每个成员各算一个）。 */
export function declaredClassNames(css: string): Set<string> {
  const names = new Set<string>();
  for (const prelude of selectorPreludes(css)) {
    for (const m of prelude.matchAll(/\.(luna-[A-Za-z0-9_-]+)/g)) names.add(m[1]);
  }
  return names;
}

/**
 * 代码里**字面**出现过的 `luna-` 类名。
 *
 * 与正向判据只认 `className=` 相反，这里**什么写法都算**：`classList.add`、
 * `querySelector(".luna-…")`、抽出去的常量、`data-*` 拼装……方向决定了宽窄（见 `ALL_CODE`）。
 * 只有两处不算消费者：注释（说到它 ≠ 用到它，剥除范围见 `stripCodeComments`），以及测试文件
 * （测试引用一个类名，不能反过来证明产品里还有人用它——`luna-hud-icon-btn` 这类退役类名
 * 正是先只剩测试与注释引用，再彻底没人用的）。
 */
export function literalClassMentions(code: Record<string, string>): Set<string> {
  const names = new Set<string>();
  for (const [path, source] of Object.entries(code)) {
    if (path.includes(".test.")) continue;
    for (const m of stripCodeComments(source).matchAll(/luna-[A-Za-z0-9_-]+/g)) names.add(m[0]);
  }
  return names;
}

/** 模板拼接的类名前缀：紧贴 `${` 的那截 `luna-…-` / `luna-…__`（下限见 ⑤ 的抬头）。 */
export function dynamicClassStems(source: string): Set<string> {
  const stems = new Set<string>();
  for (const m of stripCodeComments(source).matchAll(
    /(luna-[a-z0-9]+(?:[-_]+[a-z0-9]+)*[-_]+)\$\{/g,
  )) {
    if (m[1].slice("luna-".length).replace(/[-_]+$/, "").length >= 6) stems.add(m[1]);
  }
  return stems;
}

/** 代码里出现过的、形如标识符的字符串字面量——拼接类名的后缀只可能来自这里。 */
export function suffixLiterals(code: Record<string, string>): Set<string> {
  const out = new Set<string>();
  for (const [path, source] of Object.entries(code)) {
    if (path.includes(".test.")) continue;
    for (const m of stripCodeComments(source).matchAll(/["'`]([A-Za-z0-9][A-Za-z0-9_-]*)["'`]/g)) {
      out.add(m[1]);
    }
  }
  return out;
}

/** 样式表里有规则、却没有任何消费者的 `luna-` 类名。 */
export function unusedRuleClassNames(
  css: Record<string, string>,
  code: Record<string, string>,
): string[] {
  const used = literalClassMentions(code);
  const suffixes = suffixLiterals(code);
  const stems = new Set<string>();
  for (const [path, source] of Object.entries(code)) {
    if (path.includes(".test.")) continue;
    for (const stem of dynamicClassStems(source)) stems.add(stem);
  }
  const dead = new Set<string>();
  for (const sheet of Object.values(css)) {
    for (const name of declaredClassNames(sheet)) {
      if (used.has(name)) continue;
      const composed = [...stems].some(
        (stem) => name.startsWith(stem) && suffixes.has(name.slice(stem.length)),
      );
      if (!composed) dead.add(name);
    }
  }
  return [...dead].sort();
}

describe("样式归属守护 · 反向：样式表里有规则、却没人用", () => {
  const stemsInCode = new Set(
    Object.entries(ALL_CODE)
      .filter(([path]) => !path.includes(".test."))
      .flatMap(([, source]) => [...dynamicClassStems(source)]),
  );

  it("扫描面非空（glob 或正则失效时本用例先红，而不是让下面的断言凭空通过）", () => {
    expect(Object.keys(CSS).length).toBeGreaterThan(15);
    expect(Object.keys(ALL_CODE).length).toBeGreaterThan(100);
    expect(Object.keys(ALL_CODE).some((path) => path.includes("/packages/editor-core/src/")), "共享渲染包不在扫描面里").toBe(true);
    expect(Object.keys(CODE).some((path) => path.includes("/packages/editor-core/src/")), "共享渲染包不在扫描面里").toBe(true);
    // 判据的三个中间产物都不许是空集：任一为空，「没有死规则」就是凭空通过的。
    const declared = new Set(Object.values(CSS).flatMap((css) => [...declaredClassNames(css)]));
    expect(declared.size).toBeGreaterThan(500);
    expect(literalClassMentions(ALL_CODE).size).toBeGreaterThan(500);
    expect(stemsInCode.size).toBeGreaterThan(0);
  });

  it("没有无人使用的规则（样式表里有、全仓却找不到任何消费者 = 它只是看起来还在服役）", () => {
    const dead = unusedRuleClassNames(CSS, ALL_CODE);
    const where = (name: string) =>
      Object.keys(CSS)
        .filter((path) => declaredClassNames(CSS[path]).has(name))
        .join(", ");
    expect(
      dead,
      `这些类名有 CSS 规则、却没有任何消费者——删掉规则，或者说明它是谁的契约：\n${dead
        .map((name) => `${name} @ ${where(name)}`)
        .join("\n")}`,
    ).toEqual([]);
  });

  it("反向：种一条死规则会红，而三类假阳性一个都不许报", () => {
    // ① 种一条无人使用的规则——对着**真实**样式表种，证明它在真语料上也会红。
    const planted = {
      ...CSS,
      "../features/__planted__.css": ".luna-planted-ghost__row { color: red }",
    };
    expect(unusedRuleClassNames(planted, ALL_CODE)).toContain("luna-planted-ghost__row");

    const declared = new Set(Object.values(CSS).flatMap((css) => [...declaredClassNames(css)]));
    const found = unusedRuleClassNames(CSS, ALL_CODE);

    // ② 模板拼接族：CSS 写死变体、TSX 拼出来，按字面搜必然搜不到，但它们全是活的。
    for (const name of [
      "luna-graph-side__node-dot--chat",
      "luna-graph-side__node-dot--transcript",
      "luna-notification-host--banner",
      "luna-notification-host--toast",
    ]) {
      expect(declared, `夹具失效：${name} 已经不在样式表里了`).toContain(name);
      expect(found, `${name} 是拼接族，不该被报`).not.toContain(name);
    }

    // ③ 类型化组件 API：当前没有调用点传 tone="danger"，但那是 IconButton 的公开契约。
    for (const name of ["luna-icon-btn--sm", "luna-icon-btn--danger", "luna-icon-btn--primary"]) {
      expect(declared, `夹具失效：${name} 已经不在样式表里了`).toContain(name);
      expect(found, `${name} 是组件契约，不该被报`).not.toContain(name);
    }

    // ④ 注释里提到的类名不是规则；声明值里的字符串同理。
    expect([...declaredClassNames("/* 已删除：.luna-activity-grid */ .luna-real {}")]).toEqual([
      "luna-real",
    ]);
    expect([...declaredClassNames('.luna-real { content: ".luna-ghost-in-value"; }')]).toEqual([
      "luna-real",
    ]);
    // 反过来，嵌套块里的第一条规则**必须**取到——漏了就等于放过一条死规则。
    expect([...declaredClassNames("@media (min-width: 1px) { .luna-inside {} }")]).toEqual([
      "luna-inside",
    ]);

    // 前缀下限：认得出真前缀，拒收会把整个命名空间洗白的两种短前缀。
    expect([...dynamicClassStems("`luna-icon-btn--${density}`")]).toEqual(["luna-icon-btn--"]);
    expect([...dynamicClassStems("`luna-${mode}`")]).toEqual([]); // 正则形状挡掉
    expect([...dynamicClassStems("`luna-a-${x}`")]).toEqual([]); // 字符数下限挡掉
    // 后缀必须是代码里真实存在的字面量，前缀对了也救不了一个谁也没写过的变体。
    expect(
      unusedRuleClassNames(
        { "../a.css": ".luna-icon-btn--zzz {}" },
        { "../a.tsx": "`luna-icon-btn--${d}`" },
      ),
    ).toEqual(["luna-icon-btn--zzz"]);
    expect(
      unusedRuleClassNames(
        { "../a.css": ".luna-icon-btn--zzz {}" },
        { "../a.tsx": '`luna-icon-btn--${d}`; const d = "zzz";' },
      ),
    ).toEqual([]);

    // ⑤ 消费者一侧的注释：整行 `//` 里念到一个类名 ≠ 有人在用它。
    expect(
      unusedRuleClassNames(
        { "../a.css": ".luna-ghost-in-line-comment {}" },
        { "../a.tsx": "  // 已退役：luna-ghost-in-line-comment\nexport const x = 1;\n" },
      ),
    ).toEqual(["luna-ghost-in-line-comment"]);
    // 行尾 `//` 之后不剥：那里可能是 `"https://…"` 的斜杠，剥了会吃掉同一行的真代码
    // ——放过一条死规则只是少报一次，吃掉真代码是把一条活规则报成死的。
    expect(
      unusedRuleClassNames(
        { "../a.css": ".luna-alive-after-url {}" },
        { "../a.tsx": 'const u = "https://x"; const c = "luna-alive-after-url";\n' },
      ),
    ).toEqual([]);
  });
});
