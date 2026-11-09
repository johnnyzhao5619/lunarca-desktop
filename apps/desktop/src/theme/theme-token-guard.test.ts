/// <reference types="vite/client" />
import { describe, expect, it } from "vitest";
import { colorLiteralViolations, staticTokenReferences } from "@lunarca/lint-config/css-guards";
import { reachableStyleSources, runtimeStyleCss } from "./style-source";

/**
 * 主题 Token 守护（兑现 TDD-11 §4「Lint 阻断硬编码颜色」）。
 *
 * 铁律（TDD-11 §2.1）：组件只能消费语义 Token（`--color-*` / `--code-*` 等），
 * **绝不写死颜色字面量**。颜色字面量只允许出现在 Token「定义」处（`--x: <值>` 行），
 * 组件区（真实 CSS 属性 = 颜色字面量）一处都不许有。判据在 `@lunarca/lint-config/css-guards`
 * （网页的 `lint:css` 用同一份），这里决定扫哪张图、放行哪条登记过的例外。
 */
describe("主题 Token 守护 · 完整样式图无硬编码颜色", () => {
  // 字幕沉浸模式窄例外（TDD-11 §2.1）：叠在视频上永远暗衬白字，且暗衬 alpha 由设置
  // 运行时注入（`rgb(0 0 0 / var(--caption-*-alpha))`），无法静态 Token 化——放行引用
  // `--caption-` 的行（其黑底为刻意主题不变量，非配色债）。
  const CAPTION_EXCEPTION = /--caption-/;
  const violations = colorLiteralViolations(reachableStyleSources, CAPTION_EXCEPTION);

  it("组件区（非 Token 定义行）不出现任何颜色字面量", () => {
    expect(violations, `发现硬编码颜色（应改用语义 Token）：\n${violations.join("\n")}`).toEqual([]);
  });

  it("判据认得出组件区的颜色字面量，也放过 Token 定义行（反向验证）", () => {
    const fixture = [{ path: "fixture.css", layer: "features", text: ".a {\n  color: #fff;\n  --x: #000;\n}" }];
    expect(colorLiteralViolations(fixture)).toEqual(["fixture.css:2: color: #fff;"]);
  });
});

/**
 * 设计阶梯 Token 完整性守护（同属 TDD-15「颜色/尺寸走设计 Token」）。
 *
 * 未定义的 CSS 自定义属性会**静默失效**：整条声明被丢弃、元素退回固有尺寸，而 typecheck /
 * lint / build 全部照常通过，只在真机上表现为「某个控件莫名其妙变形」。译境控制条就栽在
 * `var(--space-7)` 上——阶梯只到 `--space-6`，两枚按钮的宽高与工具条最小高全部失效。
 *
 * `color` / `speaker` 两族是 2026-07-24 补入的：说话人配色批次里发现 `--color-bg-secondary`
 * （两处）与 `--color-accent-subtle`（一处）**从未定义**——徽标底色与队列胶囊底色一直静默
 * 失效，而 typecheck / lint / build 全绿。语义色与阶梯同属「静态声明」，没有理由豁免。
 */
describe("主题 Token 守护 · 静态阶梯无悬垂引用", () => {
  const report = staticTokenReferences(runtimeStyleCss);

  it("每个被引用的静态阶梯 Token 都有定义", () => {
    expect(report.missing, `发现悬垂 Token 引用（声明会静默失效）：\n${report.missing.join("\n")}`).toEqual([]);
  });

  it("守护本身确实看得到 Token（防正则失配后空集永远通过）", () => {
    expect(report.declared.has("--space-7")).toBe(true);
    expect(report.declared.has("--opacity-idle")).toBe(true);
    expect(report.declared.has("--color-bg-muted")).toBe(true);
    // 引用到的去重 Token 数量级（当前 83）；远低于此说明正则失配、守护已失效。
    expect(report.referenced.size).toBeGreaterThan(60);
  });

  it("判据认得出悬垂引用（反向验证）", () => {
    expect(staticTokenReferences(":root { --space-1: 1px; }\n.a { margin: var(--space-9); }").missing).toEqual([
      "--space-9（首次引用于聚合样式:2）",
    ]);
  });
});
