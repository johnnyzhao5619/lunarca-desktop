/// <reference types="vite/client" />
/**
 * 控件行高与下拉原语的守护（TDD-15 §3.5 控件行高档 / §3.3a 原语与原生外观）。
 *
 * 三件真实发生过的事，各对应一条判据：
 *
 * ① **`--control-height-*` 曾是"写在规范里但没有任何消费者"的一层**（TDD-15 §3.5 落地注
 *    2026-09-06 白纸黑字记着这件事）。没有消费者的 Token 不是抽象，是误导——它让人以为
 *    存在一个可调的控件高度档，而实际每个控件各用 `--space-*` 内边距撑各自的高。判据一
 *    因此不是"Token 存在"，是**四个控件原语都真的引用了它**。
 *
 * ② **同一行里的查询框与排序下拉差 7px**（真机 2026-09-08 上报）。根因是两者走的不是同一
 *    个盒模型：查询框是"外框 + 内部裸 input"，排序是套着 `.luna-input` 的**原生 `<select>`**
 *    ——后者忽略 `line-height`、自带最小高度，于是只能在 feature 里手调一句 `padding-block`
 *    去凑；而 `--space-*` 在舒适密度档下被重映射，凑出来的等式当场失效。判据二因此盯住
 *    **feature 不得再对这两个原语覆盖高度/纵向内边距**——等高必须由原语保证，不由手调保证。
 *
 * ③ **`<select>` 曾有七种各自为政的皮肤，且没有一处关掉原生外观**（TDD-15 §3.3a 早已写明
 *    "原生控件的默认外观必须显式关掉"）。判据三盯住**新的第八种**：任何 `<select>` 的类名
 *    要么走原语，要么在下方名单里带着理由登记在案。名单是穷尽的——删掉一处已收编的皮肤
 *    要同步删名单项，加一处新皮肤则当场判红。
 */

import { describe, expect, it } from "vitest";
import { reachableStyleSources, sourcePathLocation } from "./style-source";

const TSX = import.meta.glob("../{app,features,components}/**/*.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/** 必须消费控件行高档的控件原语：类名 → 它所在的样式文件。 */
const HEIGHT_CONSUMERS: ReadonlyArray<readonly [string, string]> = [
  [".luna-input", "components/input.css"],
  [".luna-query-box", "components/query-box.css"],
  [".luna-select", "components/select.css"],
  [".luna-btn", "components/button.css"],
];

/**
 * 等高由结构保证的一对原语：feature 不得覆盖它们的块向尺寸与纵向内边距。
 *
 * 只收这两个（而不是全部控件原语）：它们是"并排出现、必须逐像素等高"的那一对，
 * 也是事故现场。`.luna-input` 的纵向内边距仍允许被覆盖——速记捕获框的自增高镜像
 * 需要与 `::after` 逐字同排版，那是它真正私有的布局需求。
 */
const NO_HEIGHT_OVERRIDE = [".luna-query-box", ".luna-select"] as const;

const OVERRIDE_PROPS = /^\s*(min-block-size|block-size|padding-block|padding)\s*:/;

/**
 * 尚未收编进 `.luna-select` 的 `<select>` 皮肤——**每一项都要带理由**。
 *
 * 这不是豁免清单，是待办台账：收编一项就删一行，删空了这条守护自己就退化成"只允许原语"。
 */
const UNMIGRATED_SELECT_CLASSES: Readonly<Record<string, string>> = {
  // 2026-09-08 清空：HUD 两处也已收编（HUD 控件档 32px 与原语 md 档同值，两套尺度无需共处）。
  // 台账空着就是本守护的终态——"只允许原语"。
};

/**
 * 取出 `<select …>` 开标签里的 `className="…"`（不含表达式形态）。
 *
 * **没写 `className` 的 `<select>` 记为 `(bare)`**：它就是一枚裸原生控件，恰恰是本守护
 * 要拦的东西。此前这一支被 `filter(undefined)` 静默丢掉——语音 HUD 里第三枚下拉没有类名，
 * 台账上两条记录读起来像"全部登记在案"，而它从来没被看见（2026-09-08 收编时发现）。
 */
export function selectClassNames(source: string): string[] {
  // 注释里的 `<select>` 讲的是这条契约本身，不是控件；先剥掉再扫。
  const code = source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/[^\n]*/g, "$1");
  return [...code.matchAll(/<select\b[^>]*?>/gs)]
    .map((match) => {
      const tag = match[0];
      // 字面量与表达式两种写法都读：表达式里取出所有 `luna-*` 字面类名。
      const literal = /className="([^"{}]+)"/.exec(tag)?.[1];
      if (literal !== undefined) return literal;
      const expression = /className=\{([^}]*)\}/.exec(tag)?.[1];
      if (expression !== undefined) {
        const names = expression.match(/luna-[a-z0-9_-]+/g);
        return names && names.length > 0 ? names.join(" ") : "(bare)";
      }
      return "(bare)";
    })
    .flatMap((value) => value.split(/\s+/))
    .filter((name) => name.length > 0);
}

/**
 * **恰好以** `.className` 为选择器（整条选择器相等，`.x--sm` / `.x:hover` / `.y .x` 都不算）
 * 的规则块正文。
 *
 * 只查"前缀出现过"是错的：`.luna-select--sm` 里的 `var(--control-height-sm)` 会替
 * `.luna-select` 本体作答，本体把行高档删掉照样全绿（2026-09-08 评审反向夹具抓到）。
 */
export function exactRuleBodies(text: string, className: string): string[] {
  const source = text.replace(/\/\*[\s\S]*?\*\//g, "");
  const bodies: string[] = [];
  for (const match of source.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const selectors = match[1].split(",").map((part) => part.trim());
    if (selectors.includes(className)) bodies.push(match[2]);
  }
  return bodies;
}

/** 某个类的规则块里，是否出现了 `property: …` 声明。 */
export function overridesIn(text: string, className: string): number[] {
  const lines = text.split("\n");
  const hits: number[] = [];
  let depth = 0;
  let owned = false;
  lines.forEach((line, index) => {
    if (depth === 0 && line.includes("{")) {
      // 选择器行：这一块是否以目标类**为主体**（`.luna-select`、`.luna-select--sm`、
      // `.luna-select:hover` 都算；`.luna-x .luna-select` 是后代覆盖，同样算）。
      owned = new RegExp(`${className.replace(".", "\\.")}(?![\\w-])`).test(line);
    }
    if (owned && depth > 0 && OVERRIDE_PROPS.test(line)) hits.push(index + 1);
    depth += (line.match(/{/g) ?? []).length - (line.match(/}/g) ?? []).length;
    if (depth === 0) owned = false;
  });
  return hits;
}

describe("控件行高档 · Token 真的被原语消费", () => {
  it.each(HEIGHT_CONSUMERS)("%s 引用 --control-height-*", (className, file) => {
    const source = reachableStyleSources.find((entry) => entry.path.endsWith(file));
    expect(source, `样式入口未导入 ${file}`).toBeDefined();
    const hits = overridesIn(source!.text, className).length;
    expect(hits, `${className} 没有任何块向尺寸声明`).toBeGreaterThan(0);
    // 本体自己的规则块里必须引用行高档——变体（`--sm`）引用了不算数。
    const bodies = exactRuleBodies(source!.text, className);
    expect(bodies.length, `找不到恰好以 ${className} 为选择器的规则块`).toBeGreaterThan(0);
    expect(
      bodies.some((body) => /var\(--control-height-(md|sm)\)/.test(body)),
      `${className} 本体没有引用 --control-height-*`,
    ).toBe(true);
  });

  it("两档在原始阶梯与舒适密度档都有定义（漏一档 = 该档控件不跟着变高）", () => {
    const primitive = reachableStyleSources.find((entry) => entry.path.endsWith("tokens/primitive.css"));
    const density = reachableStyleSources.find((entry) => entry.path.endsWith("tokens/density.css"));
    for (const token of ["--control-height-md", "--control-height-sm"]) {
      expect(primitive!.text, `primitive 缺 ${token}`).toContain(`${token}:`);
      expect(density!.text, `舒适档缺 ${token}`).toContain(`${token}:`);
    }
  });
});

describe("等高的一对原语 · feature 不得手调高度", () => {
  const featureSources = reachableStyleSources.filter((entry) => entry.path.includes("/features/"));

  it("守护本身确实看得到 feature 样式（防空集永远通过）", () => {
    expect(featureSources.length).toBeGreaterThan(5);
  });

  it.each(NO_HEIGHT_OVERRIDE)("%s 的块向尺寸/纵向内边距不被任何 feature 覆盖", (className) => {
    const offenders = featureSources.flatMap((entry) =>
      overridesIn(entry.text, className).map((line) => sourcePathLocation(entry.path, line)),
    );
    expect(offenders, `等高应由原语保证，不由 feature 手调：\n${offenders.join("\n")}`).toEqual([]);
  });
});

describe("下拉原语 · 不得再长出第八种皮肤", () => {
  const usages = Object.entries(TSX)
    // 测试文件里的 `<select` 是断言字符串，不是控件。
    .filter(([path]) => !/\.test\.tsx?$/.test(path))
    .flatMap(([path, source]) => selectClassNames(source).map((name) => ({ path, name })));

  it("守护本身确实看得到 <select>（防正则失配后空集永远通过）", () => {
    // 终态：全应用只剩原语自己那一枚 `<select>`（`SelectBox` 里的 `.luna-select__control`）。
    // 空集 = 正则失配；看不到原语那一枚同样是失配。
    expect(usages.length).toBeGreaterThan(0);
    expect(
      usages.some(
        ({ path, name }) => path.endsWith("SelectBox.tsx") && name === "luna-select__control",
      ),
      "连 SelectBox 自己那枚 <select> 都没看到",
    ).toBe(true);
  });

  it("每个 <select> 的类要么走原语，要么在待办台账里登记在案", () => {
    const stray = usages
      .filter(({ name }) => !name.startsWith("luna-select"))
      .filter(({ name }) => !(name in UNMIGRATED_SELECT_CLASSES))
      // 纯占位类（`__sort` 之流）跟在原语类后面出现，不是皮肤；判据只看该用法里
      // 有没有原语类。
      .filter(({ path, name }) => {
        const owner = TSX[path];
        return !new RegExp(`className="[^"]*luna-select[^"]*${name}`).test(owner);
      });
    expect(
      stray.map(({ path, name }) => `${path.replace(/^\.\.\//, "src/")}: .${name}`),
      "新的 <select> 皮肤必须走 `.luna-select` 原语（TDD-15 §3.3a：原生外观必须显式关掉）",
    ).toEqual([]);
  });

  it("台账没有僵尸项（已收编的皮肤要同步删行）", () => {
    const live = new Set(usages.map(({ name }) => name));
    const zombies = Object.keys(UNMIGRATED_SELECT_CLASSES).filter((name) => !live.has(name));
    expect(zombies, "这些类已不再用于任何 <select>，从台账删除").toEqual([]);
  });
});
