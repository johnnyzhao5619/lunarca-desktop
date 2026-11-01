// @vitest-environment happy-dom
/**
 * 阅读视图里的**库内相对链接**（PRD-06-030 / ADR-026 ②(a)）。
 *
 * 真机 2026-09-09：主题页底部的「来源」区是纯文本，点不动。根因在渲染面——共享的
 * Markdown 渲染只把 `http(s)` 做成可点链接（防 scheme 注入，对），而所有**自动生成的
 * 溯源链接都是标准相对 md 链接**（蒸馏 / 汇总 / 主题页初稿逐条如此）。于是用户点不动
 * 自己刚生成的那篇产物的来源。
 *
 * 解析归 Rust（`../` 归一、越界判定、存不存在都是文件系统的事实）；这份用例钉的是
 * 前端这一半：渲染成可点的东西、把 href 原样交出去、解析不到就什么都不做。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { ReadingView } from "./ReadingView";

beforeEach(resetBindingsDouble);

const BODY = [
  "综合正文。",
  "",
  "## 来源",
  "",
  "- [cutover-迁移 汇总-1](cutover-%E8%BF%81%E7%A7%BB%20%E6%B1%87%E6%80%BB-1.md)",
  "- [站外资料](https://example.com/a)",
  "",
].join("\n");

function mount(onOpenPath = vi.fn()) {
  render(
    withI18n(
      <ReadingView content={BODY} notePath="Knowledge/主题页.md" onOpenPath={onOpenPath} onOpenTag={() => {}} />,
    ),
  );
  return { onOpenPath };
}

afterEach(cleanup);

describe("阅读视图的库内链接", () => {
  it("「来源」里的相对链接是可点的，href 原样交给 Rust 解析", async () => {
    setCommandReply("vaultLinkLocation", { status: "ok", data: { path: "Knowledge/cutover-迁移 汇总-1.md", line: null, seek_ms: null } });
    const { onOpenPath } = mount();

    const link = screen.getByText("cutover-迁移 汇总-1");
    expect(link.tagName, "纯文本就是这个 bug 本身").toBe("BUTTON");

    fireEvent.click(link);
    // 百分号编码原样交出去：链接是 Rust 写的，解也归它。
    expect(commandMock("vaultLinkLocation")).toHaveBeenCalledWith(
      "Knowledge/主题页.md",
      "cutover-%E8%BF%81%E7%A7%BB%20%E6%B1%87%E6%80%BB-1.md",
    );
    await waitFor(() => expect(onOpenPath).toHaveBeenCalledWith("Knowledge/cutover-迁移 汇总-1.md"));
  });

  it("解析不到就什么都不做——不猜一个「大概是这篇」", async () => {
    setCommandReply("vaultLinkLocation", { status: "ok", data: null });
    const { onOpenPath } = mount();
    fireEvent.click(screen.getByText("cutover-迁移 汇总-1"));
    await waitFor(() => expect(commandMock("vaultLinkLocation")).toHaveBeenCalled());
    expect(onOpenPath).not.toHaveBeenCalled();
  });

  it("拦截器拿到 href 与同一 href 的出现序号；拒绝处理时退回普通打开", async () => {
    const twice = [
      "- 甲（[周会](../Notes/周会.md)）",
      "- 乙（[复盘](../Notes/复盘.md)）",
      "- 丙（[周会](../Notes/周会.md)）",
      "",
    ].join("\n");
    const intercept = vi.fn(async (_href: string, occurrence: number) => occurrence === 1);
    const onOpenPath = vi.fn();
    setCommandReply("vaultLinkLocation", { status: "ok", data: { path: "Notes/周会.md", line: null, seek_ms: null } });
    render(
      withI18n(
        <ReadingView
          content={twice}
          notePath="Knowledge/项目.md"
          onOpenPath={onOpenPath}
          interceptVaultLink={intercept}
          onOpenTag={() => {}}
        />,
      ),
    );
    const [first, second] = screen.getAllByText("周会");
    fireEvent.click(second);
    await waitFor(() => expect(intercept).toHaveBeenCalledWith("../Notes/周会.md", 1));
    // 第二处被拦截器处理：不再按普通链接打开。
    expect(commandMock("vaultLinkLocation")).not.toHaveBeenCalled();
    fireEvent.click(first);
    await waitFor(() => expect(intercept).toHaveBeenCalledWith("../Notes/周会.md", 0));
    await waitFor(() => expect(onOpenPath).toHaveBeenCalledWith("Notes/周会.md"));
  });

  it("站外链接照旧走系统浏览器，不当成库内笔记", () => {
    mount();
    fireEvent.click(screen.getByText("站外资料"));
    expect(commandMock("openExternalUrl")).toHaveBeenCalledWith("https://example.com/a");
    expect(commandMock("vaultLinkLocation")).not.toHaveBeenCalled();
  });
});

describe("阅读面里的 [[wiki]]（真机回归）", () => {
  it("渲染成可点的 wiki 链接，不留方括号；点了按 wiki 打开", () => {
    const onActivateWikiLink = vi.fn();
    setCommandReply("vaultResolveWikilinks", { status: "ok", data: ["项目/周会.md"] });
    render(
      withI18n(
        <ReadingView content="参见 [[周会]] 一节" notePath="日记/今天.md" onOpenPath={vi.fn()} onActivateWikiLink={onActivateWikiLink} />,
      ),
    );
    const link = screen.getByRole("button", { name: "周会" });
    expect(link.className).toContain("luna-wiki-link");
    expect(screen.getByTestId("reading-view").textContent).toBe("参见 周会 一节");
    fireEvent.click(link);
    expect(onActivateWikiLink).toHaveBeenCalledWith("周会");
  });
});
