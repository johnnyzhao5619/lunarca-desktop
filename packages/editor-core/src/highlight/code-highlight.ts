// 围栏代码的语言解析与语法着色——**两个渲染面共用的一条链路**（TDD-15 §3.4a）。
//
// 语言目录基于 CodeMirror 官方 language-data 的完整目录，再补齐 Markdown 社区广泛使用、
// 但上游目录未声明的短别名。这里仅规范 Info String，不猜测拼写错误，因此 `pyhton` 之类
// 仍会诚实降级为纯文本。
//
// 编辑面把 `codeLanguageForFence` 交给 `markdown({ codeLanguages })` 做嵌套解析，CM6 自己
// 惰性加载并着色；阅读面没有 CM6，故 `highlightCodeSpans()` 把同一套东西手动跑一遍：
// 同一个语言目录 → 同一个解析器 → 同一份 `lunaHighlightStyle` → 同一批 `luna-tok-*` class。
// 零新增依赖（`@codemirror/language-data` / `@lezer/highlight` 编辑面本就在用，ADR-005）。

import { LanguageDescription } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { highlightTree } from "@lezer/highlight";
import { lunaHighlightStyle } from "./markdown-highlight";

const FENCE_LANGUAGE_ALIASES: Readonly<Record<string, string>> = {
  py: "python",
  python3: "python",
  rs: "rust",
  kt: "kotlin",
  kts: "kotlin",
  md: "markdown",
  docker: "dockerfile",
  "c-sharp": "csharp",
  objectivec: "objective-c",
  ps1: "powershell",
  pl: "perl",
  hs: "haskell",
  clj: "clojure",
};

/**
 * 解析 fenced-code 的首个 Info String token。`markdown` 本身也只以首 token 匹配，
 * 这里把别名归一到官方语言目录，返回 LanguageDescription 可继续按需惰性加载语法包。
 */
export function codeLanguageForFence(info: string): LanguageDescription | null {
  const token = info.trim().split(/\s+/u, 1)[0]?.toLowerCase();
  if (!token) {
    return null;
  }
  return LanguageDescription.matchLanguageName(
    languages,
    FENCE_LANGUAGE_ALIASES[token] ?? token,
    true,
  );
}

/** 一段被着色的代码片段：`[from, to)` 字节区间 + 该区间的语义 class。 */
export interface CodeTokenSpan {
  from: number;
  to: number;
  className: string;
}

/**
 * 对一段代码跑一次语法着色，产出 token 区间。
 *
 * **同步**且不触发加载：语法包未就绪（或语言名根本匹配不上）时返回 `null`，调用方
 * 按纯文本渲染即可——这正是"未知语言诚实降级"的那一档，与编辑面同一口径。
 * 需要加载时先 `await ensureFenceLanguage(info)`，成功后再调本函数。
 */
export function highlightCodeSpans(code: string, info: string): CodeTokenSpan[] | null {
  const support = codeLanguageForFence(info)?.support;
  if (!support) {
    return null;
  }
  const tree = support.language.parser.parse(code);
  const spans: CodeTokenSpan[] = [];
  highlightTree(tree, lunaHighlightStyle, (from, to, className) => {
    spans.push({ from, to, className });
  });
  return spans;
}

/** 该 Info String 现在就能着色吗（语言认得出且语法包已加载）。 */
export function isFenceLanguageReady(info: string): boolean {
  return codeLanguageForFence(info)?.support !== undefined;
}

/**
 * 确保某 Info String 对应的语法包已加载。返回是否可着色。
 *
 * 加载失败（离线打包缺块、动态 import 失败）与语言名匹配不上同样返回 `false`——
 * 两者对渲染面是同一件事：这块代码就按纯文本呈现，不报错、不留半渲染状态。
 */
export async function ensureFenceLanguage(info: string): Promise<boolean> {
  const desc = codeLanguageForFence(info);
  if (!desc) {
    return false;
  }
  if (desc.support) {
    return true;
  }
  try {
    await desc.load();
    return true;
  } catch {
    return false;
  }
}
