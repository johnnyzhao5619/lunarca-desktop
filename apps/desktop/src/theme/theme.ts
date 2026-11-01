// Lunar Workbench / Lunar Paper、密度与无障碍根属性控制器（TDD-11 §主题与 a11y）。
// 业务事实源在 Rust（appearance.* 设置）；此处只把已读出的设置投影到 documentElement
// 的 data-* 与 CSS 变量上，CSS 经语义 Token 响应（禁内联样式/硬编码颜色，AGENTS.md §4）。

export type ThemeMode = "system" | "light" | "dark";

/**
 * 主题族：只决定配色性格，与明暗**正交**（PRD-12-017）。取值是主题 id——内置的 `luna` / `nebula`，
 * 或内容层里导入的主题包 id（ADR-169 ④）。对不对得上由 Rust 判定（`appearance_theme` / 首帧快照
 * 已回落 `luna`），这里不再维护一张已知取值表。
 */
export type ThemeFamily = string;
/** 信息密度档（PRD-12-021）：与明暗、配色族正交。 */
export type Density = "compact" | "comfortable";

/** 与 Rust `resolve_density` 同一条回落规则：未知值一律紧凑。 */
export function resolveDensity(value: string): Density {
  return value === "comfortable" ? "comfortable" : "compact";
}

export type BodyWidth = "narrow" | "medium" | "wide";

/** 正文宽度三档（`editor.body_width`，对标方案 E）；未知值回落到默认「中」。 */
export function resolveBodyWidth(value: string): BodyWidth {
  return value === "narrow" || value === "wide" ? value : "medium";
}

/** 投影到根属性：版心 Token 按档重映射（`styles/tokens/base.css`），编辑、阅读、知识库阅读同一宽度。 */
export function applyBodyWidth(value: string): void {
  document.documentElement.dataset.bodyWidth = resolveBodyWidth(value);
}


const INSTALLED_THEME_STYLE_ID = "luna-installed-theme";

/** 安装 Rust 从封闭 palette 生成的固定变量表；空值移除本地主题投影。 */
export function applyInstalledThemeCss(css: string | null): void {
  document.getElementById(INSTALLED_THEME_STYLE_ID)?.remove();
  if (!css) return;
  const style = document.createElement("style");
  style.id = INSTALLED_THEME_STYLE_ID;
  style.textContent = css;
  document.head.append(style);
}

/** 预览前的样子（预览结束时原样放回）；没在预览为 `null`。 */
let previewRestore: (() => void) | null = null;

/**
 * 预览一套主题（PRD-18-007 / ADR-168 ⑦）：只换根节点上的族（本地主题另装它那段 CSS），**不写设置**；
 * `endThemePreview` 原样放回。纯呈现：用哪一套仍只由 `appearance.theme_family` 决定。
 */
export function previewThemeFamily(family: ThemeFamily, installedCss: string | null): void {
  endThemePreview();
  const root = document.documentElement;
  const beforeFamily = root.dataset.themeFamily ?? "luna";
  const beforeCss = document.getElementById(INSTALLED_THEME_STYLE_ID)?.textContent ?? null;
  root.dataset.themeFamily = family;
  applyInstalledThemeCss(installedCss);
  previewRestore = () => {
    root.dataset.themeFamily = beforeFamily;
    applyInstalledThemeCss(beforeCss);
  };
}

/** 结束预览，换回预览前的主题（没在预览时什么都不做）。 */
export function endThemePreview(): void {
  previewRestore?.();
  previewRestore = null;
}


export interface ThemeOptions {
  mode: ThemeMode;
  family: ThemeFamily;
  reduceMotion: boolean;
  highContrast: boolean;
  /** 字号缩放倍率（1 = 100%）。注册表以百分比整数存储，调用方除以 100 后传入。 */
  fontScale: number;
  density: Density;
}

/** 解析 system 模式下的明暗（依赖系统 prefers-color-scheme）。 */
export function resolveTheme(mode: ThemeMode): "light" | "dark" {
  if (mode !== "system") {
    return mode;
  }
  const systemDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  return systemDark ? "dark" : "light";
}

/** 把外观设置应用到 :root；reduceMotion 与系统 prefers-reduced-motion 取并集。 */
export function applyTheme({
  mode,
  family,
  reduceMotion,
  highContrast,
  fontScale,
  density,
}: ThemeOptions): void {
  const root = document.documentElement;
  root.dataset.theme = resolveTheme(mode);
  root.dataset.themeFamily = family;
  const systemReduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  root.dataset.reduceMotion = reduceMotion || systemReduceMotion ? "true" : "false";
  root.dataset.contrast = highContrast ? "high" : "normal";
  root.style.setProperty("--font-scale", String(fontScale));
  root.dataset.density = resolveDensity(density);
}
