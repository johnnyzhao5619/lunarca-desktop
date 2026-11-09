// @vitest-environment happy-dom
import { describe, expect, it, vi } from "vitest";
import { render } from "@testing-library/react";
import { MarkdownContent } from "./MarkdownContent";
import { recordingHost, withI18n } from "../test-fixture";

describe("阅读面脚注（对标方案 D3）", () => {
  it("引用是上标按钮，悬停是脚注内容，点一下滚到定义；配不上的原样显示", () => {
    const { container } = render(
      withI18n(<MarkdownContent host={recordingHost()} content={"正文[^1]与[^x]。\n\n[^1]: 第一条 **说明**"} />),
    );
    const button = container.querySelector<HTMLButtonElement>("sup.luna-footnote-ref > button")!;
    expect(button.textContent).toBe("1");
    expect(button.title).toBe("第一条 说明");
    expect(container.textContent).toContain("[^x]");
    const definition = container.querySelector<HTMLElement>(".luna-footnote[data-footnote-id='1']")!;
    expect(definition.querySelector("strong")?.textContent).toBe("说明");
    const scroll = vi.fn();
    definition.scrollIntoView = scroll;
    button.click();
    expect(scroll).toHaveBeenCalled();
  });
});
