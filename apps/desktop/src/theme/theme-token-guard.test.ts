/// <reference types="vite/client" />
import { describe, expect, it } from "vitest";
import { reachableStyleSources, runtimeStyleCss } from "./style-source";

/**
 * 主题 Token 守护（兑现 TDD-11 §4「Lint 阻断硬编码颜色」）。
 *
 * 铁律（TDD-11 §2.1）：组件只能消费语义 Token（`--color-*` / `--code-*` 等），
 * **绝不写死颜色字面量**。颜色字面量只允许出现在 Token「定义」处（`--x: <值>` 行），
 * 组件区（真实 CSS 属性 = 颜色字面量）一处都不许有。
 *
 * 本测试扫描完整样式导入图：剥离注释后，任何**非 `--自定义属性` 定义行**里出现
 * hex / rgb() / rgba() / hsl() / hsla() 字面量即判违规，并列出行号供定位。
 */
describe("主题 Token 守护 · 完整样式图无硬编码颜色", () => {
  // 剥离块注释（避免注释里的示例色值误报），保留行数以便定位。
  const COLOR_LITERAL = /#[0-9a-fA-F]{3,8}\b|\b(?:rgba?|hsla?)\(/;
  // Token 定义行：`  --foo: ...`（颜色字面量的唯一合法归属地）。
  const TOKEN_DEF = /^\s*--[\w-]+\s*:/;
  // 字幕沉浸模式窄例外（TDD-11 §2.1）：叠在视频上永远暗衬白字，且暗衬 alpha 由设置
  // 运行时注入（`rgb(0 0 0 / var(--caption-*-alpha))`），无法静态 Token 化——放行引用
  // `--caption-` 的行（其黑底为刻意主题不变量，非配色债）。
  const CAPTION_EXCEPTION = /--caption-/;

  const violations = reachableStyleSources.flatMap((source) => {
    const stripped = source.text.replace(/\/\*[\s\S]*?\*\//g, (block: string) =>
      block.replace(/[^\n]/g, " "),
    );
    return stripped
      .split("\n")
      .map((line, index) => ({ line, n: index + 1 }))
      .filter(({ line }) => COLOR_LITERAL.test(line) && !TOKEN_DEF.test(line) && !CAPTION_EXCEPTION.test(line))
      .map(({ line, n }) => `${source.path}:${n}: ${line.trim()}`);
  });

  it("组件区（非 Token 定义行）不出现任何颜色字面量", () => {
    expect(violations, `发现硬编码颜色（应改用语义 Token）：\n${violations.join("\n")}`).toEqual([]);
  });
});

/**
 * 设计阶梯 Token 完整性守护（同属 TDD-15「颜色/尺寸走设计 Token」）。
 *
 * 未定义的 CSS 自定义属性会**静默失效**：整条声明被丢弃、元素退回固有尺寸，而 typecheck /
 * lint / build 全部照常通过，只在真机上表现为「某个控件莫名其妙变形」。译境控制条就栽在
 * `var(--space-7)` 上——阶梯只到 `--space-6`，两枚按钮的宽高与工具条最小高全部失效。
 *
 * 只覆盖**静态设计阶梯与语义色**：这些 Token 恒定义在样式表内（`:root` / 主题块 / 具体
 * 规则），绝不由运行期 JS 注入，故「引用但未声明」必然是笔误，零误报。可由组件行内注入的
 * 布局变量（`--floating-menu-x`、`--sidebar-width`、`--caption-*-alpha`、`--graph-*`）不在此列。
 *
 * `color` / `speaker` 两族是 2026-07-24 补入的：说话人配色批次里发现 `--color-bg-secondary`
 * （两处）与 `--color-accent-subtle`（一处）**从未定义**——徽标底色与队列胶囊底色一直静默
 * 失效，而 typecheck / lint / build 全绿。语义色与阶梯同属「静态声明」，没有理由豁免。
 */
describe("主题 Token 守护 · 静态阶梯无悬垂引用", () => {
  const STATIC_SCALES = [
    "space",
    "opacity",
    "radius",
    "border-width",
    "outline-width",
    "font-size",
    "line-height",
    "shadow",
    "transition",
    "color",
    "speaker",
  ];
  const token = `--(?:${STATIC_SCALES.join("|")})-[a-z0-9-]+`;

  const declared = new Set(
    [...runtimeStyleCss.matchAll(new RegExp(`(?:^|[;{\\s])(${token})\\s*:`, "gm"))].map(
      (match: RegExpMatchArray) => match[1],
    ),
  );

  const referenced = new Map<string, number>();
  runtimeStyleCss.split("\n").forEach((line: string, index: number) => {
    for (const match of line.matchAll(new RegExp(`var\\((${token})\\)`, "g"))) {
      if (!referenced.has(match[1])) referenced.set(match[1], index + 1);
    }
  });

  it("每个被引用的静态阶梯 Token 都有定义", () => {
    const missing = [...referenced.entries()]
      .filter(([name]) => !declared.has(name))
      .map(([name, line]) => `${name}（首次引用于聚合样式:${line}）`);
    expect(missing, `发现悬垂 Token 引用（声明会静默失效）：\n${missing.join("\n")}`).toEqual([]);
  });

  it("守护本身确实看得到 Token（防正则失配后空集永远通过）", () => {
    expect(declared.has("--space-7")).toBe(true);
    expect(declared.has("--opacity-idle")).toBe(true);
    expect(declared.has("--color-bg-muted")).toBe(true);
    // 引用到的去重 Token 数量级（当前 83）；远低于此说明正则失配、守护已失效。
    expect(referenced.size).toBeGreaterThan(60);
  });
});
