/**
 * 公式渲染（KaTeX，MIT，含随包字体；PRD-06-011 2026-10-03 修订）。
 *
 * **按需加载**：KaTeX 与它的样式表只在页面第一次出现公式时才拉进来（动态 import 单独分包），
 * 没有公式的笔记不付这份体积。样式表由这里导入而不是走 `styles/index.css`：后者只收仓库内的
 * 受管样式，第三方样式只作用于 `.katex` 自己的类，不进 Cascade Layer 体系。
 *
 * 编辑面部件用 `katex.render` 直接构建 DOM；阅读面（React，含静态导出——导出不能等异步）用
 * `renderToString` 的结果，导出前须已 `loadKatex()`。输出由 KaTeX 生成并转义输入，`trust: false`
 * 关掉 `\href` / `\includegraphics` 一类会引用外部资源的命令；语法错误不抛，显示为红字原文。
 */

type Katex = typeof import("katex").default;

let loaded: Katex | null = null;
let loading: Promise<Katex> | null = null;

export function loadKatex(): Promise<Katex> {
  loading ??= Promise.all([import("katex"), import("katex/dist/katex.min.css")]).then(([module]) => {
    loaded = module.default;
    return loaded;
  });
  return loading;
}

/** 已加载就同步给出（静态导出用）；没加载返回 null。 */
export function katexIfLoaded(): Katex | null {
  return loaded;
}

const OPTIONS = { throwOnError: false, strict: "ignore", trust: false, output: "htmlAndMathml" } as const;

/** 渲染进一个元素（编辑面部件 / 阅读面组件）。加载前元素里保留 TeX 原文作占位。 */
export function renderMathInto(element: HTMLElement, tex: string, display: boolean): void {
  element.textContent = tex;
  void loadKatex().then((katex) => {
    katex.render(tex, element, { ...OPTIONS, displayMode: display });
  });
}

/** 静态导出：同步出 HTML 字符串；KaTeX 尚未加载时返回 null（调用方退为原文）。 */
export function renderMathToString(tex: string, display: boolean): string | null {
  return loaded ? loaded.renderToString(tex, { ...OPTIONS, displayMode: display }) : null;
}

/** 一段 Markdown 里有没有可能出现公式（导出前决定要不要先加载 KaTeX）。 */
export function mayContainMath(text: string): boolean {
  return text.includes("$");
}
