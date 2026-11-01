// Wiki-link 装饰 + 点击（PRD-06-030，Obsidian 风格 [[]] 链接）。
// 「指向哪一篇」全在 Rust（ADR-003）：本模块只把文中出现的目标批量交给 Rust，按回答着色；
// 点击把目标原文交给调用方（`openWikiLink`），打开 / 跳到片段 / 新建都由 Rust 的回答决定。

import { EditorView, Decoration, ViewPlugin, type DecorationSet, type ViewUpdate } from "@codemirror/view";
import { StateEffect, StateField, type EditorState } from "@codemirror/state";
// 「这个位置在不在围栏代码块里」全编辑器只有一份答案（`code-fence-editing.ts`）。
import { isInsideFencedCode } from "./code-fence-editing";

/** wiki-link 语法单一来源（装饰/点击/Live Preview 括号折叠/阅读视图共用）。 */
export const WIKI_LINK_RE = /\[\[([^\]|]+)(?:\|([^\]]+))?\]\]/g;

export interface WikiLinkContext {
  /** 当前笔记（解析锚点：同名笔记先取它所在目录的那篇）。 */
  notePath: () => string | null;
  /** 一批目标各自指向哪一篇（`null` = 库里没有）。 */
  resolve: (notePath: string, targets: string[]) => Promise<(string | null)[] | null>;
  /** 点击一条链接；`target` 是 `|` 之前的原文，可带 `#片段`。 */
  onActivate: (target: string) => void;
}

interface LinkMatch {
  from: number;
  to: number;
  target: string;
}

/**
 * 这处 `[[` 是不是嵌入 `![[…]]` 的一部分（D5）：嵌入不是链接——独占一行的由嵌入部件渲染，
 * 夹在文字里的照原文；当成链接装饰或折叠括号，会把 `[[ ]]` 藏掉、只剩一个孤零零的「!」
 * （真机 2026-10-04）。链接着色与实时预览的括号折叠共用这一个判断。
 */
export function isWikiEmbedAt(lineText: string, index: number): boolean {
  return index > 0 && lineText[index - 1] === "!";
}

function scanLineForWikilinks(lineText: string, lineFrom: number): LinkMatch[] {
  const out: LinkMatch[] = [];
  WIKI_LINK_RE.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = WIKI_LINK_RE.exec(lineText)) !== null) {
    const target = m[1].trim();
    if (!target) continue;
    if (isWikiEmbedAt(lineText, m.index)) continue;
    const matchStart = lineFrom + m.index;
    out.push({ from: matchStart, to: matchStart + m[0].length, target });
  }
  return out;
}

/** 扫 `ranges` 覆盖的行（缺省全文）；着色只扫可见范围，问 Rust 才扫全文（且去抖）。 */
function scanDocument(state: EditorState, ranges: readonly { from: number; to: number }[] = [{ from: 0, to: state.doc.length }]): LinkMatch[] {
  const out: LinkMatch[] = [];
  let next = 0;
  for (const range of ranges) {
    let pos = Math.max(range.from, next);
    while (pos <= range.to) {
      const line = state.doc.lineAt(pos);
      if (line.text.includes("[[")) {
        for (const m of scanLineForWikilinks(line.text, line.from)) {
          if (!isInsideFencedCode(state, m.from)) out.push(m);
        }
      }
      pos = line.to + 1;
      next = pos;
    }
  }
  return out;
}

/** Rust 的回答：目标原文 → 是否指得到。没问过的目标不在表里（按已解析呈现，免得一闪而过的「未创建」）。 */
const mergeResolutions = StateEffect.define<ReadonlyMap<string, boolean>>();
/** 库里增删改名之后，之前的回答作废，全部重问。 */
export const resetWikiResolutions = StateEffect.define<null>();

const resolutionField = StateField.define<ReadonlyMap<string, boolean>>({
  create: () => new Map(),
  update(map, tr) {
    let next = map;
    for (const effect of tr.effects) {
      if (effect.is(resetWikiResolutions)) next = new Map();
      if (effect.is(mergeResolutions)) next = new Map([...next, ...effect.value]);
    }
    return next;
  },
});

function buildDecorations(state: EditorState, ranges: readonly { from: number; to: number }[]): DecorationSet {
  const resolved = state.field(resolutionField);
  return Decoration.set(
    scanDocument(state, ranges).map((m) =>
      Decoration.mark({
        class: resolved.get(m.target) === false ? "luna-wiki-link luna-wiki-link--unresolved" : "luna-wiki-link",
      }).range(m.from, m.to),
    ),
    true,
  );
}

const RESOLVE_DELAY_MS = 250;

export function wikiLinkExtension(ctx: WikiLinkContext) {
  // 着色只扫可见范围（长文档每次按键扫全文，输入延迟随篇幅涨，对标方案 A5）。
  const decorations = ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(view: EditorView) {
        this.decorations = buildDecorations(view.state, view.visibleRanges);
      }
      update(update: ViewUpdate) {
        const answered = update.transactions.some((tr) =>
          tr.effects.some((e) => e.is(mergeResolutions) || e.is(resetWikiResolutions)),
        );
        if (update.docChanged || update.viewportChanged || answered) {
          this.decorations = buildDecorations(update.state, update.view.visibleRanges);
        }
      }
    },
    { decorations: (instance) => instance.decorations },
  );

  const resolver = ViewPlugin.fromClass(
    class {
      private timer: ReturnType<typeof setTimeout> | null = null;
      private destroyed = false;

      constructor(private readonly view: EditorView) {
        this.schedule();
      }

      update(update: ViewUpdate) {
        const reset = update.transactions.some((tr) => tr.effects.some((e) => e.is(resetWikiResolutions)));
        if (update.docChanged || reset) this.schedule();
      }

      private schedule() {
        if (this.timer !== null) clearTimeout(this.timer);
        this.timer = setTimeout(() => {
          this.timer = null;
          void this.ask();
        }, RESOLVE_DELAY_MS);
      }

      private async ask() {
        const notePath = ctx.notePath();
        if (!notePath) return;
        const known = this.view.state.field(resolutionField);
        const targets = [...new Set(scanDocument(this.view.state).map((m) => m.target))].filter(
          (target) => !known.has(target),
        );
        if (targets.length === 0) return;
        const answer = await ctx.resolve(notePath, targets);
        // 等回答期间换了笔记：这批回答属于上一篇，丢掉。
        if (this.destroyed || !answer || ctx.notePath() !== notePath) return;
        this.view.dispatch({
          effects: mergeResolutions.of(new Map(targets.map((target, i) => [target, answer[i] !== null]))),
        });
      }

      destroy() {
        this.destroyed = true;
        if (this.timer !== null) clearTimeout(this.timer);
      }
    },
  );

  return [resolutionField, decorations, resolver];
}

/** 编辑面里某个 `.luna-wiki-link` 元素对应的链接目标（`|` 之前的原文）；不是就 `null`。 */
export function wikiTargetAtElement(view: EditorView, element: Element): string | null {
  const linkEl = element.closest(".luna-wiki-link");
  if (!linkEl || !view.contentDOM.contains(linkEl)) return null;
  const pos = view.posAtDOM(linkEl);
  if (pos < 0) return null;
  const line = view.state.doc.lineAt(pos);
  return scanLineForWikilinks(line.text, line.from).find((m) => pos >= m.from && pos <= m.to)?.target ?? null;
}

/**
 * 在 mousedown 就打开，与 md 链接同一时机（`md-link.ts`）。等到 click 就晚了：mousedown 已把光标
 * 放进链接，实时预览随即把这段换回源码、原元素被替换，click 落在新 DOM 上认不出链接——
 * 真机 2026-10-04 表现为「第一下只露出源码，第二下才打开」。⌘ 点击同样打开（规格：⌘ 点击归链接）；
 * ⌥ / ⇧ / Ctrl 留给多光标、扩选与右键。
 */
export function wikiLinkClickHandler(ctx: Pick<WikiLinkContext, "onActivate">) {
  return EditorView.domEventHandlers({
    mousedown: (event, view) => {
      if (event.button !== 0 || event.altKey || event.shiftKey || event.ctrlKey) return false;
      if (!(event.target instanceof HTMLElement)) return false;
      const target = wikiTargetAtElement(view, event.target);
      if (target === null) return false;
      event.preventDefault();
      ctx.onActivate(target);
      return true;
    },
  });
}
