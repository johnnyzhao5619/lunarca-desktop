// @vitest-environment happy-dom
/// <reference types="vite/client" />
import { describe, expect, it } from "vitest";
import { applyBodyWidth, resolveBodyWidth } from "./theme";
import { runtimeStyleCss as css } from "./style-source";

describe("正文宽度三档（editor.body_width）", () => {
  it("未知值回落到默认「中」，投影到根属性", () => {
    expect(resolveBodyWidth("narrow")).toBe("narrow");
    expect(resolveBodyWidth("wide")).toBe("wide");
    expect(resolveBodyWidth("full")).toBe("medium");
    applyBodyWidth("wide");
    expect(document.documentElement.dataset.bodyWidth).toBe("wide");
    applyBodyWidth("");
    expect(document.documentElement.dataset.bodyWidth).toBe("medium");
  });

  it("窄 / 宽两档各自重映射版心 Token，默认档沿用基础值", () => {
    expect(css).toMatch(/:root\[data-body-width="narrow"\]\s*\{\s*--editor-content-max-inline:/u);
    expect(css).toMatch(/:root\[data-body-width="wide"\]\s*\{\s*--editor-content-max-inline:/u);
    expect(css).not.toMatch(/data-body-width="medium"/u);
  });
});
