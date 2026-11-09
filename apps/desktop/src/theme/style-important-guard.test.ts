import { describe, expect, it } from "vitest";
import { importantUses as importantUsesIn, stripCommentsKeepLines as withoutComments } from "@lunarca/lint-config/css-guards";
import { reachableStyleSources, sourceDisplayPath } from "./style-source";

function importantUses() {
  return importantUsesIn(reachableStyleSources).map((use) => ({ ...use, path: sourceDisplayPath(use.path) }));
}

describe("样式表守护 · !important 仅限不可替代的全局无障碍与第三方 DOM", () => {
  it("没有以优先级覆盖自有组件样式", () => {
    const uses = importantUses();
    expect(uses.map(({ path, property }) => `${path}:${property}`)).toEqual([
      // `hidden` 是平台语义，不该由某个组件的 `display` 推翻——而类选择器的特异性天然
      // 高于 `[hidden]`，于是没有这一条时它失败得完全无声（markup 在、快照断言得到、
      // 可及树也当它隐藏，屏幕上照常摆着）。这正是本清单标题里「不可替代的全局无障碍」。
      "packages/styles/src/reset.css:display",
      "packages/styles/src/tokens/accessibility.css:animation-duration",
      "packages/styles/src/tokens/accessibility.css:animation-iteration-count",
      "packages/styles/src/tokens/accessibility.css:transition-duration",
      "packages/styles/src/tokens/accessibility.css:scroll-behavior",
      "packages/styles/src/codemirror.css:font-family",
      // `.cm-content` 画布收编条：CM baseTheme 的无层 padding/margin 简写会让正文贴边，
      // 内距取 editor Token，feature 只负责 max-inline-size。
      "packages/styles/src/codemirror.css:padding-block",
      "packages/styles/src/codemirror.css:padding-inline",
      "packages/styles/src/codemirror.css:margin-inline",
      // `.cm-line` 内距收编条（§3.3d）：CM baseTheme 的无层 `padding` 简写压过一切
      // 分层 longhand；此两条是内距的唯一物理负责者，feature 侧只设 `--cm-line-pad-*`。
      "packages/styles/src/codemirror.css:padding-block",
      "packages/styles/src/codemirror.css:padding-inline",
      // 自绘光标的颜色（§3.3d）：CM baseTheme 的无层 `&light .cm-cursor` 压过层内 Token
      // 声明，深色主题下光标是黑的。（选区矩形已改由 selection-layer.ts 自绘，不再需要
      // 压 `.cm-selectionBackground`。）
      "packages/styles/src/codemirror.css:border-left-color",
      // 光标粗细与活动行底色（§3.3d）：CM baseTheme 的无层 `.cm-cursor` 1.2px 线宽与
      // `.cm-activeLine` 青蓝底压过层内 Token；深色主题下光标看不见、整段像被选中。
      "packages/styles/src/codemirror.css:border-left-width",
      "packages/styles/src/codemirror.css:background-color",
      // 装订线列归零（PRD-04-066，2026-09-08）：CM 浅色主题的无层 `.cm-gutters`
      // 灰底 / 右侧竖线 / 灰字，真机上就是"编辑器左侧意义不明的白条"。
      "packages/styles/src/codemirror.css:background",
      "packages/styles/src/codemirror.css:border",
      "packages/styles/src/codemirror.css:color",
      // 查找 / 替换面板（TDD-15 §3.3a）：CM 面板主题无层注入灰底、渐变按钮、银边输入框，
      // 每一条 important 对应 CM 确实注入了的那一条属性（`.cm-panels` 底色 / 字色 /
      // 上边线；`.cm-panel.cm-search` 内距；label 外距与字号；checkbox 外距；
      // `.cm-textfield` 外距 / 内距 / 边框 / 底色 / 字色 / 字号 / 对齐 + 聚焦边色；
      // `.cm-button` 外距 / 内距 / 边框 / 圆角 / 渐变 / 字色 / 字号 / 对齐；关闭键字号）。
      "packages/styles/src/codemirror.css:background",
      "packages/styles/src/codemirror.css:color",
      "packages/styles/src/codemirror.css:border-block-start",
      "packages/styles/src/codemirror.css:padding",
      "packages/styles/src/codemirror.css:margin",
      "packages/styles/src/codemirror.css:font-size",
      "packages/styles/src/codemirror.css:margin",
      "packages/styles/src/codemirror.css:margin",
      "packages/styles/src/codemirror.css:padding",
      "packages/styles/src/codemirror.css:border",
      "packages/styles/src/codemirror.css:background",
      "packages/styles/src/codemirror.css:color",
      "packages/styles/src/codemirror.css:font-size",
      "packages/styles/src/codemirror.css:vertical-align",
      "packages/styles/src/codemirror.css:border-color",
      "packages/styles/src/codemirror.css:margin",
      "packages/styles/src/codemirror.css:padding",
      "packages/styles/src/codemirror.css:border",
      "packages/styles/src/codemirror.css:border-radius",
      "packages/styles/src/codemirror.css:background-image",
      "packages/styles/src/codemirror.css:color",
      "packages/styles/src/codemirror.css:font-size",
      "packages/styles/src/codemirror.css:vertical-align",
      "packages/styles/src/codemirror.css:font-size",
      // 浮层外框与补全列表：`&light .cm-tooltip` 的边框 / 底色；`& > ul` 等宽字族；
      // `& > li` 内距；`&light … li[aria-selected]` 底色 / 字色。
      "packages/styles/src/codemirror.css:border",
      "packages/styles/src/codemirror.css:background-color",
      "packages/styles/src/codemirror.css:font-family",
      "packages/styles/src/codemirror.css:padding",
      "packages/styles/src/codemirror.css:background",
      "packages/styles/src/codemirror.css:color",
      // 编辑器聚焦轮廓：CM baseTheme 的无层 `&.cm-focused { outline: 1px dotted }` 压过层内 Token，
      // 网页上露出一圈虚线框（2026-11-06 真机核对）。
      "packages/styles/src/codemirror.css:outline",
    ]);
    expect(uses.every(({ line }) => line > 0)).toBe(true);
  });

  it("例外保持最小且有明确理由", () => {
    const codemirror = reachableStyleSources.find(
      (source) => source.path.endsWith("/packages/styles/src/codemirror.css"),
    )?.text;
    // CM6 baseTheme **无层**，而无层胜过任何层，
    // 故层内 important 是唯一能压住它的手段。理由必须写在归属文件里，不是"照例加个 important"。
    expect(codemirror).toContain("不在任何 cascade layer 里");

    const accessibility = reachableStyleSources.find(
      (source) => source.path.endsWith("/tokens/accessibility.css"),
    )?.text;
    expect(accessibility).toContain("减少动效");
  });

  it("守护能识别新增的优先级覆盖（反向验证）", () => {
    expect(withoutComments(".fixture { color: red !important; }")).toMatch(/!important\b/);
  });

  /**
   * 判据不许空转：那条 reset 必须真的在扫描面里。
   *
   * 它被删掉时不会有任何东西变红——`hidden` 在 markup 里照常在、快照断言照常通过、
   * 可及树也照常当它隐藏，只有屏幕上那块东西又冒出来（真机 2026-09-11 即如此）。
   */
  it("`[hidden]` 的全局隐藏规则还在", () => {
    const reset = reachableStyleSources.find((source) => source.path.endsWith("/reset.css"));
    expect(reset, "reset.css 不在扫描面里——判据已失去对象").toBeTruthy();
    expect(reset?.text.replace(/\s+/g, " ")).toContain("[hidden] { display: none !important; }");
  });
});
