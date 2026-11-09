/**
 * 西文拼写检查（笔记库对标轮 W6a）：编辑器这一侧。
 *
 * 波浪线由编辑器自己画，错词位置由 Rust（系统拼写检查器）算：停止输入一会儿后，把可见的
 * 那几屏送过去（`spelling_check`），拿回 UTF-16 区间——与 CodeMirror 的位置同一单位。不用 WebKit
 * 的「输入时检查拼写」：它只检查刚编辑过的一小段，打开一篇已有的笔记什么都不标（真机
 * 2026-10-05）。
 *
 * - 查不查、哪些算西文单词，Rust 判；这里只负责「送哪一段」与「哪些区域不查」：行内代码、
 *   代码块、网址、HTML、frontmatter。
 * - 光标正在打的那个词先不标（还没打完），光标离开后下一轮再标。
 * - 右键建议：取光标处的词交给 Rust（`spelling_suggestions`）。
 */

import { syntaxTree } from "@codemirror/language";
import { StateEffect, StateField, type EditorState, type Extension } from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin, type DecorationSet, type ViewUpdate } from "@codemirror/view";
import { commands } from "../../ipc/bindings";
import { getFrontmatterRange } from "@lunarca/editor-core/frontmatter";

/** 停止输入多久后检查（毫秒）。 */
const CHECK_DELAY_MS = 400;

/** 不做拼写检查的语法节点。 */
const NO_SPELL_NODES = new Set(["InlineCode", "FencedCode", "CodeBlock", "URL", "Autolink", "HTMLTag", "HTMLBlock", "CommentBlock"]);

interface Span {
  from: number;
  to: number;
}

function noSpellRanges(state: EditorState, from: number, to: number): Span[] {
  const ranges: Span[] = [];
  const frontmatter = getFrontmatterRange(state);
  if (frontmatter && frontmatter.from < to && frontmatter.to > from) {
    ranges.push({ from: frontmatter.from, to: frontmatter.to });
  }
  syntaxTree(state).iterate({
    from,
    to,
    enter: (node) => {
      if (!NO_SPELL_NODES.has(node.name)) return;
      if (node.to > node.from) ranges.push({ from: node.from, to: node.to });
      return false;
    },
  });
  return ranges;
}

const covered = (zones: readonly Span[], span: Span) => zones.some((zone) => zone.from <= span.from && zone.to >= span.to);

const setMisspellings = StateEffect.define<Span[]>();

/** 要求立刻重查一次（「学习拼写」之后：正文没变，但判定变了）。 */
export const recheckSpelling = StateEffect.define<null>();

const misspelledMark = Decoration.mark({ class: "luna-editor__misspelled" });

const misspellingField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(value, tr) {
    let next = value.map(tr.changes);
    for (const effect of tr.effects) {
      if (effect.is(setMisspellings)) {
        next = Decoration.set(
          effect.value.map((span) => misspelledMark.range(span.from, span.to)),
          true,
        );
      }
    }
    return next;
  },
  provide: (field) => EditorView.decorations.from(field),
});

/**
 * Rust 返回的区间（相对送去的那段文字）→ 要画的错词：换成文档位置，去掉不查的区域与光标正在
 * 打的那个词（纯函数，可测）。
 */
export function placeMisspellings(
  state: EditorState,
  offset: number,
  found: ReadonlyArray<{ start: number; end: number }>,
): Span[] {
  const head = state.selection.main.head;
  const zones = noSpellRanges(state, offset, offset + found.reduce((max, m) => Math.max(max, m.end), 0));
  return found
    .map((m) => ({ from: offset + m.start, to: offset + m.end }))
    .filter((span) => span.to <= state.doc.length && !(span.from <= head && head <= span.to) && !covered(zones, span));
}

const checker = ViewPlugin.fromClass(
  class {
    timer: ReturnType<typeof setTimeout> | null = null;
    destroyed = false;

    constructor(readonly view: EditorView) {
      this.schedule();
    }

    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        update.viewportChanged ||
        update.selectionSet ||
        update.transactions.some((tr) => tr.effects.some((effect) => effect.is(recheckSpelling)))
      ) {
        this.schedule();
      }
    }

    schedule() {
      if (this.timer !== null) clearTimeout(this.timer);
      this.timer = setTimeout(() => void this.check(), CHECK_DELAY_MS);
    }

    async check() {
      this.timer = null;
      const { view } = this;
      const doc = view.state.doc;
      const visible = view.visibleRanges;
      if (visible.length === 0) return;
      const from = doc.lineAt(visible[0].from).from;
      const to = doc.lineAt(visible[visible.length - 1].to).to;
      const found = await commands.spellingCheck(doc.sliceString(from, to));
      // 等待期间又改了：这批位置已不可信，下一轮（改动已排上）会重查。
      if (this.destroyed || view.state.doc !== doc) return;
      view.dispatch({ effects: setMisspellings.of(placeMisspellings(view.state, from, found)) });
    }

    destroy() {
      this.destroyed = true;
      if (this.timer !== null) clearTimeout(this.timer);
    }
  },
);

/** 打开拼写检查时装进编辑器的扩展（Compartment 插槽按设置热切换）。 */
export function spellcheckExtension(): Extension {
  return [misspellingField, checker];
}

/** 光标处要不要查的那个词：在不查的区域里、或光标处不是一个词时为 `null`。 */
export function spellingTargetAt(state: EditorState, pos: number): { from: number; to: number; word: string } | null {
  const word = state.wordAt(pos);
  if (!word || word.empty) return null;
  if (covered(noSpellRanges(state, word.from, word.to), word)) {
    return null;
  }
  return { from: word.from, to: word.to, word: state.sliceDoc(word.from, word.to) };
}
