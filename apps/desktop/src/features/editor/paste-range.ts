import { StateEffect, StateField, type EditorState } from "@codemirror/state";
import { isolateHistory } from "@codemirror/commands";
import type { EditorView } from "@codemirror/view";

interface PasteRange { from: number; to: number }
const setPasteRange = StateEffect.define<{ id: number; range: PasteRange | null }>();
export const pasteRangeField = StateField.define<ReadonlyMap<number, PasteRange>>({
  create: () => new Map(),
  update(current, tr) {
    const next = new Map<number, PasteRange>();
    if (!tr.isUserEvent("undo") && !tr.isUserEvent("redo")) {
      for (const [id, range] of current) {
        let overlaps = false;
        tr.changes.iterChangedRanges((from, to) => {
          if (from < range.to && to > range.from) overlaps = true;
          if (from === 0 && to === tr.startState.doc.length && to > from) overlaps = true;
        });
        if (!overlaps) {
          const from = tr.changes.mapPos(range.from, 1);
          next.set(id, { from, to: range.from === range.to ? from : Math.max(from, tr.changes.mapPos(range.to, -1)) });
        }
      }
    }
    for (const effect of tr.effects) if (effect.is(setPasteRange)) {
      if (effect.value.range) next.set(effect.value.id, effect.value.range);
      else next.delete(effect.value.id);
    }
    return next;
  },
});
let sequence = 0;
export function freezePaste(view: EditorView, range: PasteRange = view.state.selection.main): number {
  const id = ++sequence;
  const { from, to } = range;
  view.dispatch({ effects: setPasteRange.of({ id, range: { from, to } }) });
  return id;
}
export function pasteRangeOf(state: EditorState, id: number): PasteRange | null {
  return state.field(pasteRangeField, false)?.get(id) ?? null;
}
export function cancelPaste(view: EditorView, id: number) {
  view.dispatch({ effects: setPasteRange.of({ id, range: null }) });
}
export function insertFrozenPaste(view: EditorView, id: number, text: string, block = false, cursorOffset?: number): boolean {
  const range = pasteRangeOf(view.state, id);
  if (!range || !view.dom.isConnected) return false;
  if (block) {
    const before = view.state.doc.sliceString(Math.max(0, range.from - 2), range.from);
    const after = view.state.doc.sliceString(range.to, Math.min(view.state.doc.length, range.to + 2));
    text = `${before && !before.endsWith("\n\n") ? before.endsWith("\n") ? "\n" : "\n\n" : ""}${text}${after && !after.startsWith("\n\n") ? after.startsWith("\n") ? "\n" : "\n\n" : ""}`;
  }
  view.dispatch({ changes: { ...range, insert: text }, selection: { anchor: range.from + (cursorOffset ?? text.length) },
    effects: setPasteRange.of({ id, range: null }), annotations: isolateHistory.of("full"), userEvent: "input.paste" });
  return true;
}
