/**
 * Mermaid 图表渲染（MIT；PRD-06-011 2026-10-03 修订）。用的是 11.x：12.x 把 EPL-2.0 的
 * `elkjs` 拉成了直接依赖，与 AGPL v3 不兼容（ADR-005）。
 *
 * **按需加载**：Mermaid 体积大，只在页面第一次出现 ```` ```mermaid ```` 时才动态 import。
 * **安全**：`securityLevel: "strict"`——Mermaid 自己用 DOMPurify 清洗输出、禁掉图里的点击脚本与
 * HTML 标签；不发任何网络请求（字体用系统字体，图标包不加载）。结果是 SVG 文本，经宿主的 HTML 出口
 * （`HostHtml`）挂上——阅读面与编辑面的图表部件挂的是同一个 `MermaidView`。
 * **主题**：跟随应用的明暗（`<html data-theme>`），按「主题 + 源码」缓存，切换主题后重渲。
 */

type Mermaid = typeof import("mermaid").default;

let loading: Promise<Mermaid> | null = null;
let initializedTheme: string | null = null;
let sequence = 0;
const cache = new Map<string, string>();

function currentTheme(): "dark" | "default" {
  return typeof document !== "undefined" && document.documentElement.dataset.theme === "dark" ? "dark" : "default";
}

function loadMermaid(): Promise<Mermaid> {
  loading ??= import("mermaid").then((module) => module.default);
  return loading;
}

export interface MermaidResult {
  svg: string | null;
  /** 图表语法哪里不对（Mermaid 的原话，告诉用户第几行出错）；成功为 null。 */
  syntaxProblem: string | null;
}

/** 渲染一段图表源码；同主题同源码直接给缓存。 */
export async function renderMermaid(code: string): Promise<MermaidResult> {
  const theme = currentTheme();
  const key = `${theme}\n${code}`;
  const hit = cache.get(key);
  if (hit !== undefined) return { svg: hit, syntaxProblem: null };
  const mermaid = await loadMermaid();
  if (initializedTheme !== theme) {
    mermaid.initialize({ startOnLoad: false, securityLevel: "strict", theme, fontFamily: "inherit" });
    initializedTheme = theme;
  }
  try {
    const { svg } = await mermaid.render(`luna-mermaid-${++sequence}`, code);
    cache.set(key, svg);
    return { svg, syntaxProblem: null };
  } catch (error) {
    return { svg: null, syntaxProblem: error instanceof Error ? error.message : String(error) };
  }
}

/** 已渲染过就同步给出（静态导出用，导出前先 `await renderMermaid` 预热）。 */
export function mermaidIfRendered(code: string): string | null {
  return cache.get(`${currentTheme()}\n${code}`) ?? null;
}
