/// <reference types="vite/client" />
// HTML 字符串进 DOM 只有一个出口（TDD-17 §6.1 / §6.2）：网页端在 Trusted Types 下运行，包里
// 任何一处绕过 `HostHtml` 的写法都会在网页上被 CSP 拦下（或在没开 TT 的宿主上悄悄成为 XSS 面）。

import { describe, expect, it } from "vitest";

const SOURCES = import.meta.glob(["../**/*.{ts,tsx}", "!../**/*.test.{ts,tsx}"], {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/** 唯一允许出现 HTML 字符串落 DOM 写法的文件（glob 键，相对本文件）。 */
const SINK = "./HostHtml.tsx";

const SINK_PATTERNS: ReadonlyArray<[string, RegExp]> = [
  ["dangerouslySetInnerHTML", /\bdangerouslySetInnerHTML\b/],
  ["innerHTML", /\.innerHTML\b/],
  ["outerHTML", /\.outerHTML\b/],
  ["insertAdjacentHTML", /\binsertAdjacentHTML\b/],
  ["DOMParser", /\bDOMParser\b/],
  ["createContextualFragment", /\bcreateContextualFragment\b/],
  ["document.write", /\bdocument\.write(?:ln)?\b/],
  ["srcdoc", /\bsrcdoc\b/i],
];

/** 剥掉注释：注释里提到某个写法是说明，不是用法。 */
function code(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//gu, " ").replace(/(^|[^:])\/\/.*$/gmu, "$1");
}

function sinksIn(source: string): string[] {
  const body = code(source);
  return SINK_PATTERNS.filter(([, pattern]) => pattern.test(body)).map(([name]) => name);
}

describe("包内 HTML 出口只有 HostHtml 一处", () => {
  it("扫描面非空，且出口文件在场", () => {
    expect(Object.keys(SOURCES).length).toBeGreaterThan(10);
    expect(SOURCES[SINK]).toBeTypeOf("string");
    expect(Object.keys(SOURCES).some((path) => path.endsWith("/MarkdownContent.tsx"))).toBe(true);
  });

  it("出口文件之外没有任何 HTML 字符串落 DOM 的写法", () => {
    const offenders = Object.entries(SOURCES)
      .filter(([path]) => path !== SINK)
      .flatMap(([path, source]) => sinksIn(source).map((name) => `${path}: ${name}`));
    expect(offenders).toEqual([]);
  });

  it("出口本身经宿主的 toTrustedHtml，不直接拿字符串赋值", () => {
    expect(code(SOURCES[SINK])).toMatch(/__html:\s*host\.toTrustedHtml\(/);
  });

  it("反向夹具：每一种写法都会被认出来，注释里的提及不算", () => {
    for (const [name, sample] of [
      ["dangerouslySetInnerHTML", "<div dangerouslySetInnerHTML={{ __html: x }} />"],
      ["innerHTML", "el.innerHTML = x;"],
      ["outerHTML", "el.outerHTML = x;"],
      ["insertAdjacentHTML", 'el.insertAdjacentHTML("beforeend", x);'],
      ["DOMParser", 'new DOMParser().parseFromString(x, "text/html");'],
      ["createContextualFragment", "range.createContextualFragment(x);"],
      ["document.write", "document.write(x);"],
      ["srcdoc", "<iframe srcDoc={x} />"],
    ] as const) {
      expect(sinksIn(sample), name).toContain(name);
    }
    expect(sinksIn("// 这里不用 innerHTML\n/* 也不用 DOMParser */\nconst a = 1;")).toEqual([]);
    // 字符串里的 `://` 不是行注释：同一行后面的写法照样要认出来。
    expect(sinksIn('const s = "https://example.test"; el.innerHTML = s;')).toEqual(["innerHTML"]);
  });
});
