// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { remeasureContent } from "./widget-measure";

const views: EditorView[] = [];
afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
  document.body.replaceChildren();
});

describe("部件长高后重读行高", () => {
  /**
   * 守的是 CM 内部字段：`requestMeasure` 只在 contentDOM 总高变了时重读行高，短笔记里
   * `min-height: 100%` 让总高不变，必须置位这个旗标。升级 @codemirror/view 后它改名或挪走，
   * 这里先红——否则嵌入图片下方的点击会静默地落到文末。
   */
  it("置位 viewState.mustMeasureContent 并请求一次测量", () => {
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new EditorView({ parent, state: EditorState.create({ doc: "正文" }) });
    views.push(view);
    const viewState = (view as unknown as { viewState: Record<string, unknown> }).viewState;
    expect(viewState).toHaveProperty("mustMeasureContent");
    viewState.mustMeasureContent = false;
    const request = vi.spyOn(view, "requestMeasure");
    remeasureContent(view);
    expect(viewState.mustMeasureContent).toBe(true);
    expect(request).toHaveBeenCalledOnce();
  });
});
