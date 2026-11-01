/**
 * 引导卡原语守护：引导子树里**只能有一份卡骨架**。
 *
 * 整改前这份样式表里并存着六份：欢迎的承诺、算力落点、模型卡、权限行、快捷键行、
 * 第一个任务卡。六份的骨架几乎逐字相同（`padding + 细边 + 圆角 + 面背景`），而那点不同
 * 不是设计决策而是漂移——算力落点漏了 `background`，在画布底上读起来是"凹陷"，其余各步
 * 是"浮起"；快捷键行干脆没有边框、圆角还小一号。截图里两页看着不像同一个产品，源头就在
 * 这里（2026-09-12 真机）。
 *
 * 判据刻意盯**声明组合**而不是类名：类名可以随便起，而"内距 + 圆角 + （描边或底色）"
 * 同时出现在一条规则里，就是在画一张卡。名单只准变短：新出现一条就要先回答
 * "它为什么不能用 `.luna-onboarding-item`"。
 */

import { describe, expect, it } from "vitest";

const CSS = import.meta.glob("./*.css", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/**
 * 在画卡的规则：同一条声明块里既有内距、又有圆角，还有描边或底色。
 *
 * 两处刻意的排除，否则这条守护会变成噪声：
 * - **胶囊**（`--radius-full`）是徽标不是卡——「推荐」「已授权」「标准档」都是它；
 * - 选择器不以 `}` 起锚（`[^{}@]` 本就跨不过上一条规则的收尾花括号）。此前写成
 *   `(?:^|\})` 时，那个被吃掉的 `}` 同时是下一条的起点，正则不许重叠匹配，于是**每隔一条
 *   规则才看一条**——反向验证里注入的那份能抓到，真表却漏掉一半。
 */
export function cardSkeletons(css: string): string[] {
  const body = css.replace(/\/\*[\s\S]*?\*\//g, " ");
  const found: string[] = [];
  for (const match of body.matchAll(/([^{}@]+?)\s*\{([^{}]*)\}/g)) {
    const decl = match[2];
    const has = (prop: string) => new RegExp(`(?:^|;)\\s*${prop}\\s*:`).test(decl);
    const pill = /border-radius\s*:[^;]*--radius-full/.test(decl);
    if (!pill && has("padding") && has("border-radius") && (has("border") || has("background"))) {
      found.push(match[1].trim().split(/\s+/).join(" "));
    }
  }
  return found;
}

/**
 * 允许画卡的三处，逐个看过：
 * - `.luna-onboarding-card` —— 向导外壳自己，那是窗口里的那张大卡，不是内容卡；
 * - `.luna-onboarding-item` —— 本原语；
 * - `.luna-onboarding__blocked` —— 主按钮灰着的原因行，它是**告示条**（无描边、注意色底），
 *   与内容卡是两种东西，套进原语反而会读成"又一件待办"。
 */
const CARD_OWNERS = [
  ".luna-onboarding-card",
  ".luna-onboarding-item",
  ".luna-onboarding__blocked",
];

describe("引导卡 · 一份骨架", () => {
  it("扫描面非空（glob 失效时本用例先红）", () => {
    expect(Object.keys(CSS).length).toBeGreaterThan(0);
    expect(Object.values(CSS).join("").length).toBeGreaterThan(5000);
  });

  it("引导样式表里画卡的只有那三处", () => {
    const offenders = Object.entries(CSS).flatMap(([path, css]) =>
      cardSkeletons(css)
        .filter((selector) => !CARD_OWNERS.includes(selector))
        .map((selector) => `${path}: ${selector}`),
    );
    expect(
      offenders,
      `又一份卡骨架——引导里"一行一件事"只该有 .luna-onboarding-item 一种说法：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("守护认得出新加的卡骨架（反向验证）", () => {
    const injected = `
      .luna-onboarding-promise {
        display: flex;
        padding: var(--space-3);
        border: var(--border-width-thin) solid var(--color-border-subtle);
        border-radius: var(--radius-md);
      }
    `;
    expect(cardSkeletons(injected)).toEqual([".luna-onboarding-promise"]);
    // 只有内距没有圆角的普通容器不算（否则这条守护会变成噪声）。
    expect(cardSkeletons(".luna-x { padding: 4px; background: red; }")).toEqual([]);
  });
});
