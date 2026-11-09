// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { foldCode, foldedRanges } from "@codemirror/language";
import { headingFoldExtension } from "./heading-fold";
import { parseFully } from "./syntax-tree-test-fixture";

const views: EditorView[] = [];
afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
  document.body.replaceChildren();
});

function mount(doc: string) {
  const parent = document.createElement("div");
  document.body.appendChild(parent);
  const view = new EditorView({ parent, state: EditorState.create({ doc, extensions: [
    markdown({ base: markdownLanguage }), headingFoldExtension({ fold: "fold", unfold: "unfold" }),
  ] }) });
  parseFully(view);
  views.push(view);
  return view;
}

const folded = (view: EditorView) => {
  const out: string[] = [];
  foldedRanges(view.state).between(0, view.state.doc.length, (from, to) => {
    out.push(view.state.sliceDoc(from, to));
  });
  return out;
};

describe("标题与列表折叠（对标方案 B3）", () => {
  const doc = "# 一\n正文甲\n## 一.一\n正文乙\n# 二\n正文丙\n\n- 项\n  - 子项\n- 单项\n\n普通段落\n第二行";

  it("三角只给标题与带子项的列表项", () => {
    const view = mount(doc);
    const lines = [...view.dom.querySelectorAll(".luna-fold-marker")].map((el) => el.closest(".cm-line")?.textContent?.replace(/^[▾▸]/u, ""));
    expect(lines).toEqual(["# 一", "## 一.一", "# 二", "- 项"]);
  });

  it("点三角按章节折起（到下一个同级标题之前），再点展开", () => {
    const view = mount(doc);
    view.dom.querySelector<HTMLButtonElement>(".luna-fold-marker__button")!.click();
    expect(folded(view)).toEqual(["\n正文甲\n## 一.一\n正文乙"]);
    expect(view.dom.querySelector(".luna-fold-placeholder")).not.toBeNull();
    view.dom.querySelector<HTMLButtonElement>(".luna-fold-marker[data-folded] .luna-fold-marker__button")!.click();
    expect(folded(view)).toEqual([]);
  });

  it("列表项折掉子项；折叠命令（⌘⌥[）作用于光标所在的那一块", () => {
    const view = mount(doc);
    view.dispatch({ selection: { anchor: doc.indexOf("- 项") + 2 } });
    expect(foldCode(view)).toBe(true);
    expect(folded(view)).toEqual(["\n  - 子项"]);
  });
});
