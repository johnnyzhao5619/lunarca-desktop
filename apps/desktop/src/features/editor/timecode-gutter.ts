// 编辑器里与回放相关的两件事（桌面专属）：回放伴随时的当前字幕行高亮，与时间戳行的就地起播装订线
// （PRD-04-066 / ADR-118）。

import { StateEffect, StateField, type EditorState, type Extension } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, gutter, GutterMarker } from "@codemirror/view";

// ─── Cue line highlight (playback sidecar) ───────────────────────────────────

/** Dispatch this effect with a 0-based line number (or null to clear). */
export const highlightCueLineEffect = StateEffect.define<number | null>();

const cueLineMark = Decoration.line({ class: "cm-cue-active" });

/** StateField that tracks which 0-based line is highlighted for cue sync. */
const cueLineDecorationField = StateField.define<DecorationSet>({
  create() {
    return Decoration.none;
  },
  update(deco, tr) {
    deco = deco.map(tr.changes);
    for (const effect of tr.effects) {
      if (effect.is(highlightCueLineEffect)) {
        const lineNo = effect.value;
        if (lineNo === null) {
          deco = Decoration.none;
        } else {
          const line = tr.state.doc.line(Math.max(1, Math.min(lineNo + 1, tr.state.doc.lines)));
          deco = Decoration.set([cueLineMark.range(line.from)]);
        }
      }
    }
    return deco;
  },
  provide(f) {
    return EditorView.decorations.from(f);
  },
});

/** Provides CM6 decoration for cue line highlighting. Include in editor extensions. */
export function cueLineHighlightExtension(): Extension {
  return [
    cueLineDecorationField,
    EditorView.baseTheme({
      ".cm-cue-active": {
        // 走语义 Token（TDD-15 §3.2 禁硬编码色）：此前这里是两个**没有定义点**的变量名
        // 加两抹写死的黄兜底——于是"当前行是什么颜色"既不随主题也不随配色族。
        backgroundColor: "var(--editor-cue-active-surface)",
        boxShadow: "inset 2px 0 0 0 var(--editor-cue-active-border)",
      },
    }),
  ];
}

// ─── 时间戳行：就地起播（PRD-04-066 / ADR-118）─────────────────────────────────

/**
 * 正文里带时间戳的行 → 会话绝对毫秒。**表由 Rust 给**（`playback_timecode_lines`），
 * 前端既不解析 `[HH:MM:SS.mmm]` 的格式，也不自己数行（ADR-003）。
 */
export type TimecodeLineMap = ReadonlyMap<number, number>;

/** 派发它更新本编辑器认得的时间戳行表；空表 = 这一篇没有可听的行。 */
export const setTimecodeLinesEffect = StateEffect.define<TimecodeLineMap>();

const timecodeLinesField = StateField.define<TimecodeLineMap>({
  create() {
    return new Map();
  },
  update(lines, tr) {
    for (const effect of tr.effects) {
      if (effect.is(setTimecodeLinesEffect)) return effect.value;
    }
    return lines;
  },
});

/**
 * 行表本身（**常驻**，与装订线分开）。
 *
 * 它不能跟着装订线一起进隔间：装订线是按"这一篇有没有时间戳行"动态装卸的，而
 * 判断依据正是这张表。字段与写它的 effect 若在同一次事务里才被创建，新字段拿到的是
 * `create()` 的空表，那条 effect 就此丢掉——于是永远装不上装订线。
 * 本扩展不渲染任何东西，常驻的代价是一个空 Map。
 */
export function timecodeLinesExtension(): Extension {
  return [timecodeLinesField];
}

/**
 * 行首那枚播放键。**常态不可见**：光标所在行常显、指针进入装订线那一列时全列淡显、
 * 悬停到键上回满（真机 2026-09-08：一整列淡三角被读成"意义不明的白条"）。
 */
class TimecodePlayMarker extends GutterMarker {
  constructor(
    private readonly title: string,
    active: boolean,
  ) {
    super();
    // 挂在装订线元素上而不是键上：CSS 据此让光标行的键常显。
    this.elementClass = active ? "cm-timecode-gutter__line--active" : "";
  }

  override toDOM(): HTMLElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "cm-timecode-play";
    button.title = this.title;
    button.setAttribute("aria-label", this.title);
    // 图标走内联 SVG 而不是字符 ▶：字形/基线/粗细随系统字体变，与整套图标对不齐。
    button.innerHTML =
      '<svg viewBox="0 0 24 24" width="11" height="11" aria-hidden="true">' +
      '<polygon points="6 3 20 12 6 21" fill="currentColor" /></svg>';
    return button;
  }
}

export interface TimecodeGutterOptions {
  /** 跳到这一毫秒并播放。 */
  onSeek: (ms: number) => void;
  /** 播放键的可访问名与提示（i18n 由调用方给，扩展里不出现文案）。 */
  playLabel: () => string;
}

/**
 * 时间戳行的就地起播（PRD-04-066）。
 *
 * 两条通路，**都不夺走编辑语义**：
 * - 行首装订线上的播放键：唯一的可见入口，点它 = 跳到这一句；
 * - `⌘/Ctrl + 点击`该行任意处：熟手通路。普通单击仍然只是放光标——转写正文是可编辑的，
 *   把裸单击改成 seek 会让"点进去改个错别字"变成"音频跳走"。
 *
 * 判据一律查**行表**（Rust 给的）：没有时间戳的行既没有键、`⌘` 点击也不响应。
 */
export function timecodeGutterExtension(options: TimecodeGutterOptions): Extension {
  // 行表由 `timecodeLinesExtension()` 常驻提供；本扩展只在"这一篇确有时间戳行"时装上。
  const msAt = (state: EditorState, pos: number): number | undefined =>
    state.field(timecodeLinesField).get(state.doc.lineAt(pos).number);

  return [
    gutter({
      class: "cm-timecode-gutter",
      lineMarker(view, line) {
        const ms = msAt(view.state, line.from);
        if (ms === undefined) return null;
        const head = view.state.selection.main.head;
        return new TimecodePlayMarker(options.playLabel(), head >= line.from && head <= line.to);
      },
      // 行表一变（换篇 / 二次转写落盘）或光标换行，装订线要跟着重画。
      lineMarkerChange(update) {
        return (
          update.selectionSet ||
          update.transactions.some((tr) =>
            tr.effects.some((effect) => effect.is(setTimecodeLinesEffect)),
          )
        );
      },
      domEventHandlers: {
        mousedown(view, line, event) {
          const ms = msAt(view.state, line.from);
          if (ms === undefined) return false;
          options.onSeek(ms);
          event.preventDefault();
          return true;
        },
      },
    }),
    EditorView.domEventHandlers({
      mousedown(event, view) {
        if (!(event.metaKey || event.ctrlKey)) return false;
        const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
        if (pos === null) return false;
        const ms = msAt(view.state, pos);
        if (ms === undefined) return false;
        options.onSeek(ms);
        event.preventDefault();
        return true;
      },
    }),
  ];
}
