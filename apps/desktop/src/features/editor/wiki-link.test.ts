// @vitest-environment happy-dom
/**
 * wiki-link 的前端一半（PRD-06-030）：把文中目标交给 Rust、按回答着色、点击交出目标原文。
 *
 * 真机缺陷 2026-10-03：点 `[[周报]]` 固定打开库根 `周报.md`，子目录里的笔记点不开；
 * `[[周报#计划]]` 被当成一篇叫「周报#计划」的笔记。根因是前端按文件名自己判了一遍。
 * 这份用例钉住：前端不再自己判，只问、只转交。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { consumePendingEditorReveal } from "../../lib/app-events";
import { resetWikiResolutions, wikiLinkClickHandler, wikiLinkExtension } from "@lunarca/editor-core/wiki-link";
import { openWikiLink } from "./wiki-link-open";
import { desktopEditorHost } from "./editor-host";

beforeEach(() => {
  resetBindingsDouble();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
  document.body.replaceChildren();
});

function mount(doc: string, resolve: (path: string, targets: string[]) => Promise<(string | null)[] | null>) {
  const onActivate = vi.fn();
  const ctx = { notePath: () => "会议.md", onActivate };
  // 解析走编辑器端口的库级能力；这里换成用例给的应答，其余照桌面那一份。
  const host = { ...desktopEditorHost, vault: { ...desktopEditorHost.vault, resolveWikiTargets: resolve } };
  const parent = document.createElement("div");
  document.body.appendChild(parent);
  const view = new EditorView({
    parent,
    state: EditorState.create({ doc, extensions: [
      markdown({ base: markdownLanguage, addKeymap: false }),
      wikiLinkExtension(host, ctx),
      wikiLinkClickHandler(ctx),
    ] }),
  });
  return { view, onActivate };
}

function linkClasses(view: EditorView): string[] {
  return [...view.dom.querySelectorAll(".luna-wiki-link")].map((el) => el.className);
}

describe("wiki-link 着色", () => {
  it("把去重后的目标原文（含片段）一次问给 Rust，只把指不到的标成未创建", async () => {
    const resolve = vi.fn(async (_path: string, targets: string[]) =>
      targets.map((t) => (t === "缺" ? null : "项目/周报.md")),
    );
    const { view } = mount("[[周报#计划]] [[缺]] [[周报#计划|别名]]\n```\n[[代码里]]\n```", resolve);
    // 没问到之前不标「未创建」：闪一下比晚一拍更误导。
    expect(linkClasses(view).some((c) => c.includes("--unresolved"))).toBe(false);
    await vi.advanceTimersByTimeAsync(300);
    expect(resolve).toHaveBeenCalledTimes(1);
    expect(resolve).toHaveBeenCalledWith("会议.md", ["周报#计划", "缺"]);
    expect(linkClasses(view)).toEqual([
      "luna-wiki-link",
      "luna-wiki-link luna-wiki-link--unresolved",
      "luna-wiki-link",
    ]);
  });

  it("库变了就作废旧回答重问；没变的目标不重复问", async () => {
    const resolve = vi.fn(async (_path: string, targets: string[]) => targets.map(() => null));
    const { view } = mount("[[甲]]", resolve);
    await vi.advanceTimersByTimeAsync(300);
    view.dispatch({ changes: { from: view.state.doc.length, insert: " 文字" } });
    await vi.advanceTimersByTimeAsync(300);
    expect(resolve).toHaveBeenCalledTimes(1);
    view.dispatch({ effects: resetWikiResolutions.of(null) });
    await vi.advanceTimersByTimeAsync(300);
    expect(resolve).toHaveBeenCalledTimes(2);
  });
});

describe("wiki-link 点击", () => {
  it("交出 `|` 之前的目标原文，不在前端拼路径", async () => {
    const { view, onActivate } = mount("见 [[周报#下周计划|计划]]", async (_p, t) => t.map(() => "x.md"));
    await vi.advanceTimersByTimeAsync(300);
    const link = view.dom.querySelector(".luna-wiki-link");
    const press = new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 });
    link?.dispatchEvent(press);
    expect(onActivate).toHaveBeenCalledWith("周报#下周计划");
    // 在 mousedown 就接住并拦下：光标不进链接，实时预览不会先把它换回源码。
    expect(press.defaultPrevented).toBe(true);
  });

  it("⌘ 按下同样打开；⌥ / ⇧ 留给多光标与扩选", async () => {
    const { view, onActivate } = mount("见 [[周报]]", async (_p, t) => t.map(() => "x.md"));
    await vi.advanceTimersByTimeAsync(300);
    const link = view.dom.querySelector(".luna-wiki-link");
    link?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0, altKey: true }));
    link?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0, shiftKey: true }));
    expect(onActivate).not.toHaveBeenCalled();
    link?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0, metaKey: true }));
    expect(onActivate).toHaveBeenCalledWith("周报");
  });
});

describe("openWikiLink", () => {
  it("指得到：打开 Rust 给的路径，有片段就请求跳行", async () => {
    vi.useRealTimers();
    setCommandReply("vaultResolveWikilink", {
      status: "ok",
      data: { path: "项目/周报.md", exists: true, line: 3, heading_index: 1 },
    });
    const onOpenPath = vi.fn();
    await openWikiLink("会议.md", "周报#下周计划", { onOpenPath, allowCreate: true, onError: vi.fn() });
    expect(onOpenPath).toHaveBeenCalledWith("项目/周报.md");
    expect(consumePendingEditorReveal("项目/周报.md")).toMatchObject({ line: 3, headingIndex: 1 });
  });

  it("指不到：按 Rust 给的路径新建；只读面不新建", async () => {
    vi.useRealTimers();
    setCommandReply("vaultResolveWikilink", {
      status: "ok",
      data: { path: "新目录/想法.md", exists: false, line: null, heading_index: null },
    });
    setCommandReply("vaultCreate", { status: "ok", data: { path: "新目录/想法.md" } });
    const onOpenPath = vi.fn();
    await openWikiLink("会议.md", "新目录/想法", { onOpenPath, allowCreate: false, onError: vi.fn() });
    expect(commandMock("vaultCreate")).not.toHaveBeenCalled();
    await openWikiLink("会议.md", "新目录/想法", { onOpenPath, allowCreate: true, onError: vi.fn() });
    expect(commandMock("vaultCreate")).toHaveBeenCalledWith("新目录/想法.md", "note", "");
    expect(onOpenPath).toHaveBeenCalledWith("新目录/想法.md");
  });
});
