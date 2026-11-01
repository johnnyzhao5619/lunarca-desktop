/// <reference types="vite/client" />
import { describe, expect, it } from "vitest";
import { reachableStyleSources, runtimeStyleCss as cssRaw, sourceLocation } from "./style-source";

/**
 * 主题颜色只有一个来源（ADR-169 ③）。
 *
 * 内置主题是主题包数据（`crates/luna-content-schema/themes/*.toml`），由 Rust 生成
 * `tokens/themes.generated.css`；对比度门槛与「每套主题覆盖全部 Token」由 Rust 的校验与生成器保证，
 * 生成物与主题包不一致时 `cargo test` 判红。这里守的是另一头：**别的样式文件不得再手写主题色**——
 * 手写的那一份会压住或漏掉主题包里的值，而 typecheck / lint / build 全绿。
 * 引用别的 Token（高对比度档把描边换成正文色）不算手写，只拦字面颜色。
 */
describe("主题色 · 只来自主题包生成物", () => {
  const generated = reachableStyleSources.find((source) => source.path.endsWith("/tokens/themes.generated.css"));

  function tokensOf(text: string, selector: string): Set<string> {
    const open = text.indexOf("{", text.indexOf(selector));
    const body = text.slice(open + 1, text.indexOf("}", open));
    return new Set([...body.matchAll(/^\s*(--[\w-]+)\s*:/gm)].map((m) => m[1]));
  }

  it("生成物被样式入口导入，回落主题与星云各有一整套 Token（防空集永远通过）", () => {
    expect(generated, "样式入口没有导入 themes.generated.css").toBeDefined();
    expect(generated!.layer).toBe("tokens.family");
    const fallback = tokensOf(generated!.text, ':root[data-theme="dark"]');
    expect(fallback.size).toBeGreaterThan(40);
    expect(tokensOf(generated!.text, ':root[data-theme-family="nebula"][data-theme="light"]')).toEqual(fallback);
  });

  it("其余样式文件不给主题 Token 写字面颜色", () => {
    const themeTokens = tokensOf(generated!.text, ':root[data-theme="dark"]');
    const literal = /^(#[0-9a-f]{3,8}|rgba?\(|hsla?\(|oklch\()/i;
    const offenders: string[] = [];
    for (const source of reachableStyleSources) {
      if (source === generated) continue;
      source.text.split("\n").forEach((line, index) => {
        const match = /^\s*(--[\w-]+)\s*:\s*([^;]+);/.exec(line);
        if (match && themeTokens.has(match[1]) && literal.test(match[2].trim())) {
          offenders.push(`${sourceLocation(source, index + 1)} ${match[1]}`);
        }
      });
    }
    expect(offenders, `主题色要写进主题包，不能手写在样式里：\n${offenders.join("\n")}`).toEqual([]);
  });

  it("焦点环是实色的焦点色，不是半透明光晕", () => {
    expect(cssRaw).toMatch(/--color-focus-ring:\s*var\(--color-border-focus\);/);
  });
});
