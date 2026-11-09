// Markdown 交互装饰（Obsidian 对标）：
// 1) 任务列表复选框可点击切换（PRD-06-011 就地渲染 / PRD-06-014 任务列表）；
// 2) 正文内联 `#tag` 装饰 + 点击跳标签视图（PRD-06-033）。
// 均为 ViewPlugin 只装饰可见区间；文本事实源不变，切换经 dispatch 改写。

import { syntaxTree } from "@codemirror/language";
import { RangeSetBuilder, type EditorState, type Extension } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  ViewPlugin,
  type ViewUpdate,
  WidgetType,
} from "@codemirror/view";
import { scanLineForTags } from "../reading/inline-tags";

interface Range {
  from: number;
  to: number;
}

// ─── 任务复选框 ──────────────────────────────────────────────────────────────

class TaskCheckboxWidget extends WidgetType {
  constructor(private readonly checked: boolean) {
    super();
  }

  override eq(other: TaskCheckboxWidget) {
    return other.checked === this.checked;
  }

  toDOM() {
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = this.checked;
    box.className = "luna-md-task-checkbox";
    // 状态由文档文本驱动；阻止浏览器自翻转，交给 mousedown 改写文本。
    box.addEventListener("click", (event) => event.preventDefault());
    return box;
  }

  override ignoreEvent() {
    return false;
  }
}

/** 构建任务复选框装饰（state 级纯函数，可无头测试）。 */
export function buildTaskDecorations(
  state: EditorState,
  ranges: readonly Range[],
): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  for (const { from, to } of ranges) {
    syntaxTree(state).iterate({
      from,
      to,
      enter(node) {
        if (node.name !== "TaskMarker") {
          return;
        }
        const text = state.sliceDoc(node.from, node.to);
        const checked = /[xX]/u.test(text);
        builder.add(
          node.from,
          node.to,
          Decoration.replace({ widget: new TaskCheckboxWidget(checked) }),
        );
      },
    });
  }
  return builder.finish();
}

/** 在 `pos` 处查找 TaskMarker，给出翻转 `[ ]`/`[x]` 的变更（state 级纯函数）。 */
export function taskToggleChangeAt(
  state: EditorState,
  pos: number,
): { from: number; to: number; insert: string } | null {
  let change: { from: number; to: number; insert: string } | null = null;
  syntaxTree(state).iterate({
    from: Math.max(0, pos - 1),
    to: Math.min(state.doc.length, pos + 1),
    enter(node) {
      if (change || node.name !== "TaskMarker") {
        return;
      }
      const text = state.sliceDoc(node.from, node.to);
      const checked = /[xX]/u.test(text);
      change = { from: node.from, to: node.to, insert: checked ? "[ ]" : "[x]" };
    },
  });
  return change;
}

/** 在 `pos` 处翻转任务复选框。返回是否命中。 */
export function toggleTaskAt(view: EditorView, pos: number): boolean {
  const change = taskToggleChangeAt(view.state, pos);
  if (!change) {
    return false;
  }
  view.dispatch({ changes: change, userEvent: "input" });
  return true;
}

/** 任务列表复选框扩展：`- [ ]` 渲染为可点击复选框，点击就地改写文本。 */
export function taskCheckboxExtension(): Extension {
  return [
    ViewPlugin.fromClass(
      class {
        decorations: DecorationSet;

        constructor(view: EditorView) {
          this.decorations = buildTaskDecorations(view.state, view.visibleRanges);
        }

        update(update: ViewUpdate) {
          if (update.docChanged || update.viewportChanged) {
            this.decorations = buildTaskDecorations(
              update.view.state,
              update.view.visibleRanges,
            );
          }
        }
      },
      { decorations: (plugin) => plugin.decorations },
    ),
    EditorView.domEventHandlers({
      mousedown: (event, view) => {
        if (
          !(event.target instanceof HTMLElement) ||
          !event.target.classList.contains("luna-md-task-checkbox")
        ) {
          return false;
        }
        const pos = view.posAtDOM(event.target);
        if (pos < 0) {
          return false;
        }
        event.preventDefault();
        return toggleTaskAt(view, pos);
      },
    }),
  ];
}

// ─── 内联 #tag ───────────────────────────────────────────────────────────────

const inlineTagMark = Decoration.mark({ class: "luna-inline-tag" });

/** 这个位置是不是在代码里（行内 code 或围栏块）——**全编辑器一份**，装饰面共用。 */
export function isCodeContext(state: EditorState, pos: number): boolean {
  const node = syntaxTree(state).resolveInner(pos, 1);
  for (let cur: typeof node | null = node; cur; cur = cur.parent) {
    if (/Code/u.test(cur.name)) {
      return true;
    }
  }
  return false;
}

/** 构建内联 #tag 装饰（state 级纯函数，可无头测试）。 */
export function buildTagDecorations(
  state: EditorState,
  ranges: readonly Range[],
): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  for (const { from, to } of ranges) {
    let pos = from;
    while (pos <= to) {
      const line = state.doc.lineAt(pos);
      for (const match of scanLineForTags(line.text)) {
        const start = line.from + match.from;
        if (!isCodeContext(state, start + 1)) {
          builder.add(start, line.from + match.to, inlineTagMark);
        }
      }
      pos = line.to + 1;
    }
  }
  return builder.finish();
}

/** 内联 `#tag` 装饰 + 点击回调（跳标签视图）。 */
export function inlineTagExtension(onOpenTag: (tag: string) => void): Extension {
  return [
    ViewPlugin.fromClass(
      class {
        decorations: DecorationSet;

        constructor(view: EditorView) {
          this.decorations = buildTagDecorations(view.state, view.visibleRanges);
        }

        update(update: ViewUpdate) {
          if (update.docChanged || update.viewportChanged) {
            this.decorations = buildTagDecorations(
              update.view.state,
              update.view.visibleRanges,
            );
          }
        }
      },
      { decorations: (plugin) => plugin.decorations },
    ),
    EditorView.domEventHandlers({
      click: (event, view) => {
        if (!(event.target instanceof HTMLElement)) {
          return false;
        }
        const el = event.target.closest(".luna-inline-tag");
        if (!(el instanceof HTMLElement)) {
          return false;
        }
        const pos = view.posAtDOM(el);
        if (pos < 0) {
          return false;
        }
        const line = view.state.doc.lineAt(pos);
        const match = scanLineForTags(line.text).find(
          (m) => pos >= line.from + m.from && pos <= line.from + m.to,
        );
        if (!match) {
          return false;
        }
        event.preventDefault();
        onOpenTag(match.tag);
        return true;
      },
    }),
  ];
}
