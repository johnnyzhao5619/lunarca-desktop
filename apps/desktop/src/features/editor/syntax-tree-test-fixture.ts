/**
 * 编辑器用例共用：拿到**语法树已解析完整**的状态 / 视图，再交给读 `syntaxTree(state)` 的产品代码。
 *
 * 只调 `ensureSyntaxTree(state, len, ms)` 不够：`syntaxTree(state)` 读的是状态建好那一刻留下的
 * 快照，`ensureSyntaxTree` 只推进背后共用的解析上下文并返回那棵新树，快照不变。用例之所以平时
 * 能过，是新建状态那 20 ms 的首轮解析通常就把短文档做完了；CPU 吃紧（全量闸门与 cargo 构建并发）
 * 时首轮只做完一小截，产品代码读到的仍是半棵树，用例偶发失败。
 *
 * 补法是解析完再过一笔空事务：`LanguageState.apply` 发现上下文的树比快照新，就换上完整的那棵。
 * 视图用 `forceParsing`（它内部正是「解析到位 + 空 dispatch」）。
 *
 * `starvedState` 把「首轮只做完一小截」钉成必走：用它建状态的用例在闲机上也走忙机器那条路，
 * 漏了 `fullyParsed` 就每次都红，而不是偶发。
 */

import { vi } from "vitest";
import { ensureSyntaxTree, forceParsing } from "@codemirror/language";
import { EditorState, type EditorStateConfig } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";

/** 解析上限（毫秒）：只兜极端情况，用例文档都很短。 */
const PARSE_BUDGET_MS = 5000;

/** 首轮解析被饿死的状态：建状态期间每读一次时钟跳 1 秒，20 ms 预算只够做一小截。 */
export function starvedState(config: EditorStateConfig): EditorState {
  let now = 0;
  const clock = vi.spyOn(Date, "now").mockImplementation(() => (now += 1000));
  try {
    return EditorState.create(config);
  } finally {
    clock.mockRestore();
  }
}

/** 语法树解析到文末、且 `syntaxTree()` 读得到完整树的状态。 */
export function fullyParsed(state: EditorState): EditorState {
  if (!ensureSyntaxTree(state, state.doc.length, PARSE_BUDGET_MS)) {
    throw new Error("syntax tree did not finish parsing within the test budget");
  }
  return state.update({}).state;
}

/** 让视图当前状态的语法树解析到文末（必要时派发一笔空事务换上完整的树）。 */
export function parseFully(view: EditorView): void {
  if (!forceParsing(view, view.state.doc.length, PARSE_BUDGET_MS)) {
    throw new Error("syntax tree did not finish parsing within the test budget");
  }
}
