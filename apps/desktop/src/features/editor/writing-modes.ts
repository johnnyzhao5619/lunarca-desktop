// 写作视图模式（PRD-06-014，Typora 对标）：
// - 打字机模式：键入/删除时把光标行滚动到编辑器垂直居中（点击/翻页不抢滚动，
//   与 Typora 语义一致）；附 scrollPastEnd 让文末也能居中。
// - 专注模式：纯 CSS——`.luna-editor__surface[data-focus]` 淡化非活动行
//   （CM 逻辑行 = Markdown 段落，恰是 Typora 的段落级专注），无需 CM 扩展。
// 两模式均经设置 `editor.typewriter_mode` / `editor.focus_mode` 持久化（设备级），
// 编辑器头部 toggle 写设置、settings://changed 驱动全部 pane 同步。

import type { Extension } from "@codemirror/state";
import { EditorView, scrollPastEnd } from "@codemirror/view";

/** 打字机居中滚动扩展（Compartment 插槽按设置热切换）。 */
export function typewriterScrolling(): Extension {
  return [
    scrollPastEnd(),
    EditorView.updateListener.of((update) => {
      const typed = update.transactions.some(
        (tr) => tr.isUserEvent("input") || tr.isUserEvent("delete"),
      );
      if (!update.docChanged || !typed) {
        return;
      }
      const view = update.view;
      const head = update.state.selection.main.head;
      // updateListener 内不可同步 dispatch（重入约束）：微任务后置居中滚动。
      queueMicrotask(() => {
        view.dispatch({
          effects: EditorView.scrollIntoView(head, { y: "center" }),
        });
      });
    }),
  ];
}
