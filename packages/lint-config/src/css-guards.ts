/**
 * CSS 守护的判据（TDD-15 §3 / §5 的 L3–L6）：纯函数，读 `style-graph.ts` 给出的样式图。
 * 桌面与网页的 `lint:css` 各自对自己的入口调用同一份判据——判据只写一次，宿主只决定看哪张图、
 * 放行哪些登记过的例外。每条判据的反向夹具由调用方的守护测试随门禁执行。
 */

import type { StyleSource } from "./style-graph";

/** 剥掉块注释但保留换行（行号不漂）。 */
export function stripCommentsKeepLines(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, (comment) => comment.replace(/[^\n]/g, " "));
}

// ── L4：颜色字面量只许出现在 Token 定义行 ─────────────────────────────────────

const COLOR_LITERAL = /#[0-9a-fA-F]{3,8}\b|\b(?:rgba?|hsla?)\(/;
const TOKEN_DEF = /^\s*--[\w-]+\s*:/;

/**
 * 组件区（非 `--自定义属性` 定义行）里的颜色字面量，逐条 `path:行: 原文`。
 * `allowLine` 是调用方登记过的窄例外（例：字幕沉浸模式的运行时注入暗衬）。
 */
export function colorLiteralViolations(sources: readonly StyleSource[], allowLine?: RegExp): string[] {
  return sources.flatMap((source) =>
    stripCommentsKeepLines(source.text)
      .split("\n")
      .map((line, index) => ({ line, n: index + 1 }))
      .filter(({ line }) => COLOR_LITERAL.test(line) && !TOKEN_DEF.test(line) && !(allowLine?.test(line) ?? false))
      .map(({ line, n }) => `${source.path}:${n}: ${line.trim()}`),
  );
}

/**
 * 静态阶梯与语义色 Token：恒定义在样式表内、绝不由运行期 JS 注入，故「引用但未声明」必然是笔误。
 * 可由组件行内注入的布局变量（`--sidebar-width` 之类）不在此列。
 */
export const STATIC_TOKEN_SCALES = [
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
] as const;

export interface TokenReferenceReport {
  declared: Set<string>;
  /** Token → 聚合样式里首次引用的行号。 */
  referenced: Map<string, number>;
  /** 被引用却没有定义的，逐条 `名字（首次引用于聚合样式:行）`。 */
  missing: string[];
}

export function staticTokenReferences(runtimeCss: string): TokenReferenceReport {
  const token = `--(?:${STATIC_TOKEN_SCALES.join("|")})-[a-z0-9-]+`;
  const declared = new Set(
    [...runtimeCss.matchAll(new RegExp(`(?:^|[;{\\s])(${token})\\s*:`, "gm"))].map((match) => match[1]),
  );
  const referenced = new Map<string, number>();
  runtimeCss.split("\n").forEach((line, index) => {
    for (const match of line.matchAll(new RegExp(`var\\((${token})\\)`, "g"))) {
      if (!referenced.has(match[1])) referenced.set(match[1], index + 1);
    }
  });
  const missing = [...referenced.entries()]
    .filter(([name]) => !declared.has(name))
    .map(([name, line]) => `${name}（首次引用于聚合样式:${line}）`);
  return { declared, referenced, missing };
}

// ── L5：方向属性用逻辑语义 ───────────────────────────────────────────────────

const PHYSICAL_PROPERTY =
  /(?:^|[;{])\s*(?:left|right|top|bottom|margin-left|margin-right|padding-left|padding-right|border-left|border-right|border-top|border-bottom|inset-left|inset-right)\s*:/gm;

export function physicalDirectionLocations(css: string, sourcePath = "fixture.css") {
  const withoutComments = stripCommentsKeepLines(css);
  return [...withoutComments.matchAll(PHYSICAL_PROPERTY)].map((match) => ({
    property: match[0].replace(/^[;{]\s*/, "").trim().slice(0, -1),
    line: (withoutComments.slice(0, match.index).match(/\n/g) ?? []).length + 1,
    sourcePath,
  }));
}

// ── L3：一个选择器只有一个专属定义块（TDD-15 §3.3b） ──────────────────────────

export type CssRule = { selector: string; atContext: string; line: number; sourcePath: string };

/**
 * 够用的 CSS 切分：只需要「块的前导选择器 + 所处 at-rule 上下文 + 行号」。判据必须**解析**而不是
 * 按行正则：多行分组选择器的最后一行长得和专属块的开头一模一样。
 */
export function parseCssRules(css: string, sourcePath: string): CssRule[] {
  const rules: CssRule[] = [];
  const atStack: string[] = [];
  let prelude = "";
  let line = 1;
  let i = 0;
  const countNewlines = (s: string) => (s.match(/\n/g) ?? []).length;

  while (i < css.length) {
    const ch = css[i];
    if (ch === "/" && css[i + 1] === "*") {
      const end = css.indexOf("*/", i + 2);
      const stop = end === -1 ? css.length : end + 2;
      line += countNewlines(css.slice(i, stop));
      i = stop;
      continue;
    }
    // 字符串（url("…{…}") 之类不能被当成块边界）
    if (ch === '"' || ch === "'") {
      let j = i + 1;
      while (j < css.length && css[j] !== ch) j += css[j] === "\\" ? 2 : 1;
      prelude += css.slice(i, j + 1);
      line += countNewlines(css.slice(i, j + 1));
      i = j + 1;
      continue;
    }
    if (ch === "{") {
      const head = prelude.trim();
      if (head.startsWith("@")) {
        atStack.push(head.replace(/\s+/g, " "));
      } else if (head) {
        rules.push({
          selector: head.replace(/\s+/g, " "),
          atContext: atStack.join(" > "),
          line: line - countNewlines(prelude) + countNewlines(prelude.replace(/^\s*/, "")),
          sourcePath,
        });
        // 声明块内部不再有嵌套块：直接跳到配对的 }
        let depth = 1;
        let j = i + 1;
        while (j < css.length && depth > 0) {
          if (css[j] === "{") depth += 1;
          else if (css[j] === "}") depth -= 1;
          j += 1;
        }
        line += countNewlines(css.slice(i, j));
        prelude = "";
        i = j;
        continue;
      }
      prelude = "";
      i += 1;
      continue;
    }
    if (ch === "}") {
      atStack.pop();
      prelude = "";
      i += 1;
      continue;
    }
    if (ch === "\n") line += 1;
    prelude += ch;
    i += 1;
  }
  return rules;
}

/** 专属块 = 逗号列表里只有一个选择器，且该选择器带类（本仓库组件层的写法）。 */
export function dedicatedBlocks(rules: readonly CssRule[]): Map<string, CssRule[]> {
  const byKey = new Map<string, CssRule[]>();
  for (const rule of rules) {
    if (rule.selector.includes(",")) continue;
    if (!rule.selector.includes(".")) continue;
    const key = `${rule.atContext}||${rule.selector}`;
    byKey.set(key, [...(byKey.get(key) ?? []), rule]);
  }
  return byKey;
}

// ── `!important` 只许出现在登记过的地方 ──────────────────────────────────────

export function importantUses(sources: readonly StyleSource[]) {
  return sources.flatMap((source) => {
    const text = stripCommentsKeepLines(source.text);
    return [...text.matchAll(/([\w-]+)\s*:\s*[^;{}]+!important\b/g)].map((match) => ({
      path: source.path,
      property: match[1],
      line: (text.slice(0, match.index).match(/\n/g) ?? []).length + 1,
    }));
  });
}

// ── CodeMirror 自带类名的物理内距（TDD-15 §3.3d） ─────────────────────────────

/**
 * 分层样式表里对 `.cm-line` 声明内距物理属性的规则（§3.3d，2026-08-03 裁字事故）：
 * CM6 baseTheme 的无层 `padding: 0 2px 0 6px` 压过**任何**分层 longhand——声明会静默
 * 失效；若同一规则还有配套的负 `text-indent`（无人竞争、独自生效），首行直接被拉出
 * 左缘裁字。唯一合法通道是设 `--cm-line-pad-*` 变量（物理属性由
 * `@lunarca/styles/codemirror.css` 的收编条单点负责）。
 */
export function layeredCmLinePadding(css: string): string[] {
  const found: string[] = [];
  const code = css.replace(/\/\*[\s\S]*?\*\//g, " ");
  for (const match of code.matchAll(/(?:^|\})\s*([^{}]*\.cm-line[^{}]*)\{([^}]*)\}/g)) {
    const decl = match[2];
    for (const prop of decl.matchAll(/(?:^|;)\s*(padding[\w-]*)\s*:/g)) {
      found.push(`${match[1].trim().slice(0, 60)} → ${prop[1]}`);
    }
  }
  return found;
}

/** `.cm-content` 的物理内距 / 外距同样只能由 CM 收编条负责。 */
export function layeredCmContentBox(css: string): string[] {
  const found: string[] = [];
  const code = css.replace(/\/\*[\s\S]*?\*\//g, " ");
  for (const match of code.matchAll(/(?:^|\})\s*([^{}]*\.cm-content[^{}]*)\{([^}]*)\}/g)) {
    const decl = match[2];
    for (const prop of decl.matchAll(/(?:^|;)\s*((?:padding|margin)[\w-]*)\s*:/g)) {
      found.push(`${match[1].trim().slice(0, 60)} → ${prop[1]}`);
    }
  }
  return found;
}
