// @vitest-environment happy-dom
/**
 * 编辑面里标准 md 链接的可点性（PRD-06-030）。
 *
 * 真机 2026-09-09：同一篇产物，阅读模式里「来源」点得动，切回编辑模式就点不动。
 *
 * 这里钉两件在真实浏览器之外也验得了的事：折叠态挂上了那个**只在折叠态存在**的抓手类，
 * 以及源码露出来时它不在场——后者正是"点一下想改 URL 不会变成跳走了"的判据。点击本身
 * 要真实 DOM 与 CodeMirror 视图，归真机。
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble } from "../../ipc/bindings-test-double";
import { buildLivePreviewDecorations, livePreviewExtension } from "@lunarca/editor-core/live-preview";
import { mdLinkClickHandler } from "@lunarca/editor-core/md-link";
import { desktopEditorHost } from "./editor-host";

beforeEach(resetBindingsDouble);

const DOC = "见 [汇总](../Knowledge/%E6%B1%87%E6%80%BB.md) 一文。";

function classesAt(cursor: number): string[] {
  const state = EditorState.create({
    doc: DOC,
    selection: { anchor: cursor },
    extensions: [markdown({ base: markdownLanguage })],
  });
  const set = buildLivePreviewDecorations(state, [{ from: 0, to: state.doc.length }]);
  const out: string[] = [];
  const iter = set.iter();
  while (iter.value) {
    const spec = iter.value.spec as { class?: string };
    if (spec.class) {
      out.push(spec.class);
    }
    iter.next();
  }
  return out;
}

describe("编辑面的 md 链接", () => {
  it("折叠态挂上点击抓手", () => {
    // 光标在别处 → `[`、`](url)` 折起来，剩下的那段文字是可点的链接。
    expect(classesAt(0)).toContain("luna-md-linkbody");
  });

  it("源码露出来时抓手不在场——那时点一下是要改 URL，不是要跳走", () => {
    expect(classesAt(DOC.indexOf("汇总"))).not.toContain("luna-md-linkbody");
  });
});

/**
 * 真视图上的端到端：装上 live preview 与本处理器，对着折叠出来的那段文字按下鼠标。
 *
 * 这一段回答的是"为什么必须挂 `mousedown`"——`click` 那一刻光标已经移进链接、源码已经
 * 展开，那个只在折叠态存在的抓手从 DOM 上没了（真机 2026-09-09：编辑模式下点来源毫无
 * 反应）。下面第二条用例把这个时序差钉住。
 */
describe("真视图上的点击", () => {
  function mountView(doc: string, opened: string[]) {
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    return new EditorView({
      parent,
      state: EditorState.create({
        doc,
        extensions: [
          markdown({ base: markdownLanguage }),
          livePreviewExtension(desktopEditorHost, () => "Notes/当前.md"),
          mdLinkClickHandler(desktopEditorHost, {
            notePath: () => "Notes/当前.md",
            onOpenPath: (path) => opened.push(path),
          }),
        ],
      }),
    });
  }

  it("按在折叠出来的那段文字上 → 交给 Rust 解析，并掐掉光标移动", () => {
    const opened: string[] = [];
    const view = mountView(DOC, opened);
    const body = view.dom.querySelector(".luna-md-linkbody");
    expect(body, "折叠态该有这个抓手").not.toBeNull();

    const event = new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 });
    body!.dispatchEvent(event);

    expect(commandMock("vaultLinkLocation")).toHaveBeenCalledWith(
      "Notes/当前.md",
      "../Knowledge/%E6%B1%87%E6%80%BB.md",
    );
    expect(event.defaultPrevented, "光标不该跳进链接中间").toBe(true);
    view.destroy();
  });

  it("到了 `click` 那一刻抓手已经不在——这正是它必须挂 `mousedown` 的原因", () => {
    const view = mountView(DOC, []);
    // 模拟 CodeMirror 在 mousedown 时把光标移进链接：源码展开，折叠态的抓手消失。
    const pos = DOC.indexOf("汇总");
    view.dispatch({ selection: { anchor: pos } });
    expect(view.dom.querySelector(".luna-md-linkbody")).toBeNull();
    view.destroy();
  });
});
