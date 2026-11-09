// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Compartment, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { frontmatterFoldingExtension, toggleFrontmatterEffect } from "@lunarca/editor-core/frontmatter";
import { desktopEditorHost } from "./editor-host";

const labels = {
  label: (count: number) => `属性 ${count}`,
  expandTitle: "展开",
  collapseTitle: "折叠",
  expandAction: "展开",
  collapseAction: "折叠",
};
const onEditSource = vi.fn();
const views: EditorView[] = [];

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("noteProperties", { status: "ok", data: [] });
});
afterEach(() => {
  views.splice(0).forEach((view) => view.destroy());
  document.body.replaceChildren();
});

function mount(doc: string) {
  const parent = document.createElement("div");
  document.body.appendChild(parent);
  const view = new EditorView({ parent, state: EditorState.create({ doc, extensions: [frontmatterFoldingExtension(desktopEditorHost, labels, { onEditSource })] }) });
  views.push(view);
  return view;
}

describe("frontmatter 胶囊展开 = 属性面板（ADR-139 ⑤）", () => {
  it("默认折叠；展开后整段换成面板，原始 YAML 不再出现在正文里", () => {
    const view = mount("---\ntitle: 周会\n---\n正文");
    expect(view.dom.querySelector(".luna-frontmatter-toggle.collapsed")).not.toBeNull();
    view.dispatch({ effects: toggleFrontmatterEffect.of() });
    expect(view.dom.querySelector(".luna-frontmatter-panel")).not.toBeNull();
    expect(view.contentDOM.textContent).not.toContain("title: 周会");
    expect(view.contentDOM.textContent).toContain("正文");
  });

  it("改正文不碰面板；改 frontmatter 时面板原地更新而不是重挂（正在输入的框不丢焦点）", () => {
    const view = mount("---\ntitle: 周会\n---\n正文");
    view.dispatch({ effects: toggleFrontmatterEffect.of() });
    const panel = view.dom.querySelector(".luna-frontmatter-panel");
    view.dispatch({ changes: { from: view.state.doc.length, insert: "！" } });
    expect(view.dom.querySelector(".luna-frontmatter-panel")).toBe(panel);
    view.dispatch({ changes: { from: 11, to: 13, insert: "例会" } });
    expect(view.state.doc.toString()).toContain("title: 例会");
    expect(view.dom.querySelector(".luna-frontmatter-panel")).toBe(panel);
  });
});

describe("模式切换（真机回归：源码 → 实时预览白屏）", () => {
  it("这组扩展被摘掉再装回来（源码 ⇄ 实时预览）不抛错，胶囊照常出现", () => {
    const presentation = new Compartment();
    const ext = () => frontmatterFoldingExtension(desktopEditorHost, labels, { onEditSource });
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new EditorView({
      parent,
      state: EditorState.create({ doc: "---\ntitle: 周会\n---\n正文", extensions: [presentation.of(ext())] }),
    });
    views.push(view);
    view.dispatch({ effects: presentation.reconfigure([]) });
    expect(view.dom.querySelector(".luna-frontmatter-toggle")).toBeNull();
    expect(() => view.dispatch({ effects: presentation.reconfigure(ext()) })).not.toThrow();
    expect(view.dom.querySelector(".luna-frontmatter-toggle.collapsed")).not.toBeNull();
  });
});
