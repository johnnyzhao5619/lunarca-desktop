// 编辑面的通用编辑手感（PRD-06-014）：列表 / 引用续写、字数、括号配对、Tab 缩进与查找键、
// 实时预览里让位的结构脚手架注释。frontmatter 折叠在 `frontmatter.ts`，撤销历史在 `editor-history.ts`。

import { indentLess, indentMore } from "@codemirror/commands";
import { markdownLanguage } from "@codemirror/lang-markdown";
import { openSearchPanel } from "@codemirror/search";
import { EditorSelection, StateField, type EditorState, type Extension } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, keymap } from "@codemirror/view";
import { isInsideFencedCode } from "./code-fence-editing";
import type { EditorHost } from "./editor-host";
import { codeMirrorKey, editorShortcut } from "./editor-shortcuts";
import { frontmatterCloseLine } from "./frontmatter";
import { parentIndentOf, parseListLine, splitQuote, withRenumber } from "./list-editing";
import { singleCursorOnly } from "./multi-cursor";

export interface ListContinuation {
  from: number;
  to: number;
  insert: string;
  cursor: number;
}

/** 各选区合计的字数（多光标时相加，B6）；没有选中任何文字返回 null。 */
export function selectionWordCount(state: EditorState): number | null {
  if (state.selection.ranges.every((range) => range.empty)) return null;
  return state.selection.ranges.reduce((sum, range) => sum + wordCount(state.sliceDoc(range.from, range.to)), 0);
}

export function wordCount(text: string) {
  const cjkMatches = text.match(/[\u3400-\u9fff\uf900-\ufaff]/gu);
  const latinMatches = text.match(/[A-Za-z0-9]+(?:['-][A-Za-z0-9]+)*/gu);
  return (cjkMatches?.length ?? 0) + (latinMatches?.length ?? 0);
}

export function listContinuationForState(state: EditorState): ListContinuation | null {
  const selection = state.selection.main;
  if (!selection.empty) {
    return null;
  }
  // 代码内容必须原样输入；Markdown 列表续写不能劫持以 `-`/`1.` 开头的代码行。
  if (isInsideFencedCode(state, selection.head)) {
    return null;
  }

  const line = state.doc.lineAt(selection.head);
  const linePrefix = line.text.slice(0, selection.head - line.from);
  const item = parseListLine(linePrefix);
  if (!item) {
    // 引用块（此前由 lang-markdown 自带键位承担，见 list-editing.ts 抬头）：
    // `> 文字|` 回车续一行 `> `；空的 `> |` 回车退出引用。
    const { quote, rest } = splitQuote(linePrefix);
    if (quote.length === 0) {
      return null;
    }
    if (rest.trim() === "") {
      return { from: line.from, to: selection.head, insert: "", cursor: line.from };
    }
    const insert = `\n${quote}`;
    return {
      from: selection.head,
      to: selection.head,
      insert,
      cursor: selection.head + insert.length,
    };
  }
  const lineBody = line.from + item.quote.length;
  if (item.body.length === 0) {
    // 嵌套的空项：回车降一级而不是退出列表（Notion / Obsidian 同）——用户多半是想
    // 回到父级继续写；降到顶层再按一次才真正退出。
    if (item.indent.length > 0) {
      const step = item.indent.length - parentIndentOf(state, line.number, item.indent.length);
      return {
        from: lineBody,
        to: lineBody + step,
        insert: "",
        cursor: selection.head - step,
      };
    }
    // 引用里的顶层空项：去掉标记、留在引用里（`> - |` → `> |`）。
    if (item.quote.length > 0) {
      return { from: lineBody, to: selection.head, insert: "", cursor: lineBody };
    }
    // 顶层空项：去掉标记并换行——留下的那条空行是刻意的，`- a\n正文` 里的正文是
    // 上一项的懒续行（CommonMark），中间没有空行就退不出列表。
    return {
      from: line.from,
      to: selection.head,
      insert: "\n",
      cursor: line.from + 1,
    };
  }

  const marker =
    item.number === null ? item.marker : `${item.number + 1}${item.marker.slice(-1)}`;
  const insert = `\n${item.quote}${item.indent}${marker} ${item.task}`;
  return {
    from: selection.head,
    to: selection.head,
    insert,
    cursor: selection.head + insert.length,
  };
}

function continueList(view: EditorView) {
  const continuation = listContinuationForState(view.state);
  if (!continuation) {
    return false;
  }

  // 在有序列表中间插项 / 嵌套空项降级之后，后续编号跟着走（与插项合成一个撤销步）。
  view.dispatch(
    withRenumber(
      view.state,
      { from: continuation.from, to: continuation.to, insert: continuation.insert },
      EditorSelection.single(continuation.cursor),
    ),
  );
  return true;
}

/**
 * Markdown 语言级括号配对声明（PRD-06-014，Typora 对标）：CM `closeBrackets`
 * 默认集缺**反引号**（markdown 行内代码高频），经语言 data 补齐。
 * 引号/反引号在**字词字符之后不自动配对**是 CM 既定策略（防 `don't`、所有格
 * 与中文语句内误配），非缺陷、不覆盖——在空白/行首输入即配对。
 */
export function markdownCloseBracketsData(): Extension {
  return markdownLanguage.data.of({
    closeBrackets: { brackets: ["(", "[", "{", "'", '"', "`"] },
  });
}

export function editorProductivityExtensions(host: EditorHost): Extension {
  return keymap.of([
    {
      key: "Enter",
      run: singleCursorOnly(continueList),
    },
    {
      key: "Tab",
      run: indentMore,
    },
    {
      key: "Shift-Tab",
      run: indentLess,
    },
    {
      key: "Mod-Alt-f",
      run: openSearchPanel,
    },
    // 「Tab 键移动焦点」：切宿主会话级那一份（`EditorHost.toggleTabFocusMode`）。CM 默认键表里的 ⇧⌥M 只切本编辑器、与状态栏对不上，
    // 故同一处一并接管（键盘布局让它能按出来时也走同一份）。
    ...[codeMirrorKey(editorShortcut("tab_focus_mode")), "Shift-Alt-m"].map((key) => ({
      key,
      run: () => {
        host.toggleTabFocusMode();
        return true;
      },
    })),
  ]);
}

/**
 * 一篇笔记的字数：只数正文，frontmatter 不算。状态栏与字数目标（W6b）都用它——此前整篇
 * 一起数，设个目标（`luna:` 块多一行）字数就凭空涨了几个（真机 2026-10-05）。
 */
export function noteWordCount(content: string): number {
  const lines = content.split("\n");
  const close = frontmatterCloseLine((line) => lines[line - 1] ?? "", lines.length);
  return wordCount(close === null ? content : lines.slice(close).join("\n"));
}

/**
 * 整行是否为 Lunarca 自己的**结构脚手架注释**（`<!-- luna:… -->` 独占一行）。
 *
 * **按 `luna:` 命名空间判定，刻意不复制具体哨兵串**。转写块的定界符是
 * `luna_vault::vault::TRANSCRIPT_BEGIN` / `TRANSCRIPT_END`，对话笔记的出处与补充标记在
 * `ipc::chat` 另有几个——把那几个常量抄一份到前端，就是给「同一个判据」再造一个所有者，
 * 而 `luna_vault::parse::is_scaffold_line` 的文档正是在警告这件事（两处口径分家的代价）。
 * 命名空间才是稳定契约：Rust 改某个哨兵叫什么，这条规则依然成立。
 *
 * **必须整行恰好是一个完整注释**：用户在正文里写 `见 <!-- luna:transcript --> 那一段` 是内容，
 * 不该被吞掉——与 Rust 侧「必须整行精确匹配」是同一条纪律。
 */
export function isLunaScaffoldComment(text: string): boolean {
  const trimmed = text.trim();
  if (!trimmed.startsWith("<!--") || !trimmed.endsWith("-->") || trimmed.length < 7) {
    return false;
  }
  // 注释内部不得再出现 `-->`：那意味着这一行有两个注释（或一个注释加正文），不是纯脚手架。
  const inner = trimmed.slice(4, -3);
  if (inner.includes("-->")) {
    return false;
  }
  const payload = inner.trim();
  return payload.startsWith("luna:") || payload.startsWith("/luna:");
}

export function buildScaffoldDecos(state: EditorState): DecorationSet {
  const decos = [];
  for (let i = 1; i <= state.doc.lines; i++) {
    const line = state.doc.line(i);
    if (isLunaScaffoldComment(line.text)) {
      // 与 frontmatter 折叠同一种范围写法（行首→行尾，`block`）：整行让位，不留占位文字。
      decos.push(Decoration.replace({ block: true }).range(line.from, line.to));
    }
  }
  return Decoration.set(decos, true);
}

/**
 * 实时预览里隐藏 Lunarca 的结构脚手架注释。
 *
 * Rust 侧把这些哨兵注释描述为「HTML 注释，渲染不可见」——那个假设**只在渲染视图成立**，
 * 而实时预览面同时也是编辑面，于是 `<!-- luna:transcript -->` 这行机器脚手架就直接摊在
 * 用户眼前（真机可见：任何一篇录音转写笔记的正文第一行）。
 *
 * **只在实时预览隐藏，源码模式照常显示**：本扩展与 frontmatter 折叠同处
 * `buildPresentationExtensions`。真要修一个坏掉的转写块时，总得有一个面能看见定界符；
 * 两个面都藏就没有退路了。
 *
 * **文件字节一个都不动**：这纯粹是渲染层的事。哨兵仍旧照写、照旧被 Rust 用来整块替换
 * 转写正文——它们是承重的，不是可以清理掉的历史垃圾。
 */
export function lunaScaffoldHidingExtension(): Extension {
  return StateField.define<DecorationSet>({
    create(state) {
      return buildScaffoldDecos(state);
    },
    update(decos, tr) {
      if (!tr.docChanged) return decos;
      // 只重查改动碰到的那几行（块级替换必须走 StateField，但不必每键扫全文，对标方案 A5）。
      let next = decos.map(tr.changes);
      tr.changes.iterChangedRanges((_fromA, _toA, fromB, toB) => {
        const from = tr.state.doc.lineAt(fromB).from;
        const to = tr.state.doc.lineAt(toB).to;
        const add = [];
        for (let pos = from; pos <= to; ) {
          const line = tr.state.doc.lineAt(pos);
          if (isLunaScaffoldComment(line.text)) add.push(Decoration.replace({ block: true }).range(line.from, line.to));
          pos = line.to + 1;
        }
        next = next.update({ filterFrom: from, filterTo: to, filter: () => false, add });
      });
      return next;
    },
    provide(f) {
      return EditorView.decorations.from(f);
    },
  });
}
