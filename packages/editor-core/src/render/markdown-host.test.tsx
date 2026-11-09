// @vitest-environment happy-dom
// 宿主端口的行为契约（TDD-17 §2 / §6.2）：外链放不放行、怎么打开、代码复制按钮、HTML 出口都交宿主。
import { describe, expect, it } from "vitest";
import { fireEvent, render } from "@testing-library/react";
import { MarkdownContent } from "./MarkdownContent";
import { recordingHost, t, withI18n } from "../test-fixture";

const LINKS = "[甲](https://a.test) [乙](http://b.test) [丙](mailto:c@d.test) [丁](javascript:alert(1))";

describe("外链交宿主判定与打开", () => {
  it("桌面口径（http/https）：放行的做成链接，点一下交宿主打开；其余是纯文本", () => {
    const host = recordingHost();
    const { container } = render(withI18n(<MarkdownContent host={host} content={LINKS} />));
    const anchors = [...container.querySelectorAll("a")].map((a) => a.getAttribute("href"));
    expect(anchors).toEqual(["https://a.test", "http://b.test"]);
    fireEvent.click(container.querySelector("a")!);
    expect(host.opened).toEqual(["https://a.test"]);
    expect(container.textContent).toContain("丙");
    expect(container.textContent).toContain("丁");
  });

  it("宿主收紧（网页：https 与 mailto）：http 与 javascript 都退成纯文本", () => {
    const host = recordingHost({ isExternalUrlAllowed: (href) => /^(?:https:|mailto:)/i.test(href) });
    const { container } = render(withI18n(<MarkdownContent host={host} content={LINKS} />));
    const anchors = [...container.querySelectorAll("a")].map((a) => a.getAttribute("href"));
    expect(anchors).toEqual(["https://a.test", "mailto:c@d.test"]);
  });

  it("库内相对链接有打开通道时交给它，外链判定仍先于它", () => {
    const opened: Array<[string, number]> = [];
    const { container } = render(
      withI18n(
        <MarkdownContent
          host={recordingHost()}
          content={"[甲](https://a.test) [乙](../Notes/乙.md)"}
          openVaultLink={(href, occurrence) => opened.push([href, occurrence])}
        />,
      ),
    );
    expect(container.querySelectorAll("a")).toHaveLength(1);
    fireEvent.click(container.querySelector<HTMLButtonElement>("button.luna-markdown__link")!);
    expect(opened).toEqual([["../Notes/乙.md", 0]]);
  });

  it("带 scheme 的链接不进库内通道：宿主不放行的 mailto: / javascript: 即使有库内通道也是纯文本", () => {
    const opened: string[] = [];
    const { container } = render(
      withI18n(
        <MarkdownContent
          host={recordingHost()}
          content={"[丙](mailto:c@d.test) [丁](javascript:alert(1)) [戊](JavaScript:alert(2))"}
          openVaultLink={(href) => opened.push(href)}
        />,
      ),
    );
    expect(container.querySelectorAll("a, button")).toHaveLength(0);
    expect(container.textContent).toContain("丙");
    expect(container.textContent).toContain("丁");
    expect(container.textContent).toContain("戊");
    expect(opened).toEqual([]);
  });
});

describe("代码块", () => {
  it("复制按钮由宿主给，拿到的是去掉围栏的代码正文；无语言时标签取包内文案", () => {
    const { container } = render(
      withI18n(<MarkdownContent host={recordingHost()} content={"```\nlet a = 1\n```"} />),
    );
    expect(container.querySelector("[data-copy]")?.getAttribute("data-copy")).toBe("let a = 1");
    expect(container.querySelector(".luna-markdown-codeblock__header")?.textContent).toBe(t("markdown.plainText"));
  });
});
