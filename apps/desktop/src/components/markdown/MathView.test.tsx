// @vitest-environment happy-dom
import { describe, expect, it } from "vitest";
import { render, waitFor } from "@testing-library/react";
import { renderToStaticMarkup } from "react-dom/server";
import { MathView } from "./MathView";
import { MarkdownContent } from "./MarkdownContent";
import { loadKatex } from "../../lib/math-render";
import { withI18n } from "../../i18n/i18n-test-fixture";

describe("公式渲染（真实 KaTeX，按需加载）", () => {
  it("先显示 TeX 原文，KaTeX 加载完换成排版结果", async () => {
    const { container } = render(<MathView tex="x^2" display={false} />);
    await waitFor(() => expect(container.querySelector(".katex")).not.toBeNull());
    expect(container.querySelector("annotation")?.textContent).toBe("x^2");
  });

  it("阅读面：行内与成块公式；预热后静态渲染（导出）直接是排好的结果；\\href 不生成链接", async () => {
    await loadKatex();
    const html = renderToStaticMarkup(
      withI18n(<MarkdownContent content={"质能 $E=mc^2$\n\n$$\n\\href{https://x.test}{y}\n$$"} />),
    );
    expect(html).toContain("katex");
    expect(html).toContain("luna-math--block");
    expect(html).not.toContain("https://x.test\"");
  });
});
