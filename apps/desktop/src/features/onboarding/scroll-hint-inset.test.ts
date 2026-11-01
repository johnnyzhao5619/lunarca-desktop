/**
 * 引导内容区的可滚提示阴影**不贴边**守护。
 *
 * 不能滚时那两道 `scroll` 层阴影靠 `local` 层盖片盖住；内容区上下沿落在半像素上时，WebKit
 * 里两层取整不一致，阴影最深的那一行漏出来，在「使用这个位置」下方画出一道淡线，与底部导航的
 * 分隔线叠成两条（2026-10-21 真机截图）。修法是阴影各往里收一根发丝线，理由写在
 * `onboarding.css` 那条规则的注释里。本条盯住修法本身：谁把阴影改回 `0 0` / `0 100%`，这里红。
 */

import { describe, expect, it } from "vitest";

import css from "./onboarding.css?raw";

/** 取某条规则（去注释后）里 `background` 的各层里，以 `scroll` 收尾的那几层。 */
export function scrollLayers(source: string, selector: string): string[] {
  const body = source.replace(/\/\*[\s\S]*?\*\//g, " ");
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const rule = new RegExp(`(?:^|\\})\\s*${escaped}\\s*\\{([^{}]*)\\}`).exec(body);
  const background = rule ? /(?:^|;)\s*background\s*:([^;]*)/.exec(rule[1]) : null;
  if (!background) {
    return [];
  }
  // 按顶层逗号切层（gradient 括号里的逗号不算）。
  const layers: string[] = [];
  let depth = 0;
  let current = "";
  for (const char of background[1]) {
    if (char === "(") depth += 1;
    if (char === ")") depth -= 1;
    if (char === "," && depth === 0) {
      layers.push(current.trim());
      current = "";
    } else {
      current += char;
    }
  }
  layers.push(current.trim());
  return layers.filter((layer) => /\bscroll$/.test(layer)).map((layer) => layer.replace(/\s+/g, " "));
}

/** 层的位置段（gradient 之后、`/` 之前）。 */
function position(layer: string): string {
  // 渐变的收尾 = 第一个回到深度 0 的右括号（位置段里的 `var(...)` 也带括号，不能取最后一个）。
  let depth = 0;
  let end = 0;
  for (let i = 0; i < layer.length; i += 1) {
    if (layer[i] === "(") depth += 1;
    if (layer[i] === ")") {
      depth -= 1;
      if (depth === 0) {
        end = i + 1;
        break;
      }
    }
  }
  return layer.slice(end).split("/")[0].trim();
}

function insetFromEdges(layers: string[]): boolean {
  return layers.length === 2 && layers.every((layer) => position(layer).includes("--border-width-thin"));
}

describe("引导内容区 · 可滚提示阴影不贴边", () => {
  it("两道阴影都往里收了一根发丝线", () => {
    const layers = scrollLayers(css, ".luna-onboarding__content");
    expect(layers).toHaveLength(2);
    expect(insetFromEdges(layers)).toBe(true);
  });

  it("反向夹具：贴边的旧写法会被判红", () => {
    const old = `.luna-onboarding__content {
      background:
        linear-gradient(var(--color-bg-panel) 40%, transparent) 0 0 / 100% var(--space-5) no-repeat local,
        linear-gradient(transparent, var(--color-bg-panel) 60%) 0 100% / 100% var(--space-5) no-repeat local,
        linear-gradient(color-mix(in srgb, var(--color-text-primary) 10%, transparent), transparent)
          0 0 / 100% var(--space-1) no-repeat scroll,
        linear-gradient(transparent, color-mix(in srgb, var(--color-text-primary) 10%, transparent))
          0 100% / 100% var(--space-1) no-repeat scroll;
    }`;
    expect(scrollLayers(old, ".luna-onboarding__content")).toHaveLength(2);
    expect(insetFromEdges(scrollLayers(old, ".luna-onboarding__content"))).toBe(false);
    // 只在注释里写对的写法喂不饱它。
    expect(insetFromEdges(scrollLayers(`/* 0 var(--border-width-thin) */${old}`, ".luna-onboarding__content"))).toBe(false);
  });
});
