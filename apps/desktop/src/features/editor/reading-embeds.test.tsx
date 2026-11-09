// @vitest-environment happy-dom
/**
 * 嵌入 `![[…]]`（对标方案 D5）在阅读面与导出里的呈现：指向什么全听 Rust。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import type { EmbedHitDto } from "../../ipc/bindings";
import { ReadingView } from "./ReadingView";
import { renderNoteHtml } from "./note-export-html";

const CONTENT = "![[周会#结论]]\n\n![[图.png|300]]\n\n附件见 ![[报告.pdf]]\n\n![[没有]]";
const HITS: EmbedHitDto[] = [
  { kind: "note", path: "项目/周会.md", content: "## 结论\n\n上线了。\n\n![[更深]]" },
  { kind: "file", path: "项目/assets/图.png", link: "../项目/assets/图.png", image: true },
  { kind: "file", path: "项目/报告.pdf", link: "../项目/报告.pdf", image: false },
  { kind: "missing" },
];

afterEach(cleanup);
beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("vaultResolveEmbeds", { status: "ok", data: HITS });
  setCommandReply("vaultResolveWikilinks", { status: "ok", data: [] });
  setCommandReply("vaultAssetPath", { status: "error", error: "not in test" });
});

describe("阅读面里的嵌入", () => {
  it("笔记成块显示内容；嵌入里的嵌入只是链接；附件是可点的名字；指不到照原文", async () => {
    const onOpenPath = vi.fn();
    render(withI18n(createElement(ReadingView, { content: CONTENT, notePath: "日记/今天.md", onOpenPath })));
    expect(await screen.findByText("上线了。")).toBeTruthy();
    expect(commandMock("vaultResolveEmbeds")).toHaveBeenCalledTimes(1);
    expect(commandMock("vaultResolveEmbeds")).toHaveBeenCalledWith("日记/今天.md", ["周会#结论", "图.png", "报告.pdf", "没有"]);
    expect(screen.getByText("更深").tagName).toBe("BUTTON");
    screen.getByText("报告.pdf").click();
    expect(onOpenPath).toHaveBeenCalledWith("项目/报告.pdf");
    expect(screen.getByText("![[没有]]").getAttribute("title")).toBe(t("editor-core:reading.embedMissing"));
  });

  it("导出：预先解析好的嵌入展开进 HTML，图片是相对宿主的路径（交给 Rust 内联）", () => {
    const html = renderNoteHtml(CONTENT, "日记/今天.md", new Map(["周会#结论", "图.png", "报告.pdf", "没有"].map((target, i) => [target, HITS[i]])));
    expect(html).toContain("上线了。");
    expect(html).toContain('src="../项目/assets/图.png"');
    expect(html).toContain('width="300"');
    expect(html).not.toContain("<button");
    expect(commandMock("vaultResolveEmbeds")).not.toHaveBeenCalled();
  });
});
