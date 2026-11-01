// AI 会话的编辑器侧状态（PRD-06-020/021）。三件事都只是「把事实画出来 / 记住位置」，
// 不含任何业务判定（ADR-003）：
//
// ① **冻结的输入范围随文档变更映射**。面板打开时记下选区 `[from, to)`；生成期间用户仍可
//    编辑正文（busy 只冻结面板，不锁编辑器），此前这段范围是两个裸数字，等待时在上方
//    插入一段文字，「替换」就会替换到错误位置。现在它是一个 `StateField`，每笔变更经
//    `changes.mapPos` 跟着位移——与朗读高亮 `deco.map` 同一机制。
// ② **落地后的渐隐高亮**。替换 / 插入完成后把新文标一下再淡出（Notion 同做法），让用户
//    看得见「改了哪里」；装饰随后续编辑自动跟随。
// ③ **键位**。⌘J 打开 / 关闭、Esc 停止或关闭、⌘↩ 接受——接受与拒绝刻意不对称
//    （Enter 只提交指令），避免「接受」变成条件反射。
//
// 另附两个**纯切片**函数：选区前后段落与光标前文。它们只是原样切文本，取不取、取多少、
// 怎么拼全在 Rust（`editor.floating_context` 与每侧上限）。

import { StateEffect, StateField, type EditorState, type Extension, type Text } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, keymap } from "@codemirror/view";

/** 文档 UTF-16 位置区间 `[from, to)`。 */
export interface AiRange {
  from: number;
  to: number;
}

/** 设置 / 清除会话冻结范围。 */
export const setAiRange = StateEffect.define<AiRange | null>();

const aiRangeField = StateField.define<AiRange | null>({
  create() {
    return null;
  },
  update(range, tr) {
    let next = range;
    if (next && tr.docChanged) {
      // 起点贴后（1）、终点贴前（-1）：恰在范围边缘插入的文字不被吞进范围。
      const from = tr.changes.mapPos(next.from, 1);
      next = { from, to: Math.max(from, tr.changes.mapPos(next.to, -1)) };
    }
    for (const effect of tr.effects) {
      if (effect.is(setAiRange)) {
        next = effect.value;
      }
    }
    return next;
  },
});

/** 此刻的冻结范围（已随变更映射；未设置时 `null`）。 */
export function aiRangeOf(state: EditorState): AiRange | null {
  return state.field(aiRangeField, false) ?? null;
}

/** 落地高亮：标记新落地的范围（`null` = 清除）。 */
export const flashLandedRange = StateEffect.define<AiRange | null>();

const landedMark = Decoration.mark({ class: "luna-ai-landed" });
const landedLine = Decoration.line({ class: "luna-ai-landed-line" });

const landedField = StateField.define<DecorationSet>({
  create() {
    return Decoration.none;
  },
  update(deco, tr) {
    let next = deco.map(tr.changes);
    for (const effect of tr.effects) {
      if (!effect.is(flashLandedRange)) {
        continue;
      }
      const value = effect.value;
      if (value === null) {
        next = Decoration.none;
        continue;
      }
      const doc = tr.state.doc;
      const from = Math.max(0, Math.min(value.from, doc.length));
      const to = Math.max(from, Math.min(value.to, doc.length));
      if (to <= from) {
        next = Decoration.none;
        continue;
      }
      // 行装饰 + 行内 mark：Live Preview 把某些文字换成部件后 mark 看不见，整行仍看得见。
      const ranges = [];
      const lastLine = doc.lineAt(to).number;
      for (let n = doc.lineAt(from).number; n <= lastLine; n += 1) {
        ranges.push(landedLine.range(doc.line(n).from));
      }
      ranges.push(landedMark.range(from, to));
      next = Decoration.set(ranges, true);
    }
    return next;
  },
  provide: (field) => EditorView.decorations.from(field),
});

/** 会话状态扩展（冻结范围 + 落地高亮），挂进 VaultEditor 的扩展集。 */
export function aiSessionExtension(): Extension {
  return [aiRangeField, landedField];
}

export interface AiSessionKeyHandlers {
  /** Esc：生成中先停止，否则关闭；无会话时返回 `false` 让其它 Esc 绑定接手。 */
  escape: () => boolean;
  /** ⌘↩：按建议方式接受结果；无结果时返回 `false`。 */
  accept: () => boolean;
}

/**
 * 会话键位（Esc 与 ⌘↩）。须排在 CM 默认 keymap **之前**（Esc 才轮得到我们）。
 *
 * 开合键（默认 `⌘J`）**不在这里**：它是可改键的设置（W3b），由编辑器的按键判定读真值后经
 * 编辑器命令执行——键位表在编辑器创建时就定死了，改键后要重建编辑器才生效。
 */
export function aiSessionKeymap(handlers: AiSessionKeyHandlers): Extension {
  return keymap.of([
    { key: "Escape", run: () => handlers.escape() },
    { key: "Mod-Enter", run: () => handlers.accept() },
  ]);
}

/** 前端切片上限（字符）：只为不把整篇都送过 IPC；真正的取舍上限在 Rust。 */
export const CONTEXT_SLICE_MAX_CHARS = 1_000;
export const CURSOR_SLICE_MAX_CHARS = 3_000;

function isBlankLine(text: string): boolean {
  return text.trim().length === 0;
}

/**
 * 选区所在段落的前文与后文（以空行为段落边界）。前文取离选区最近的尾部，后文取头部。
 * 选区本身不在结果里。
 */
export function contextAround(doc: Text, from: number, to: number): { before: string; after: string } {
  const docLength = doc.length;
  const start = Math.max(0, Math.min(from, docLength));
  const end = Math.max(start, Math.min(to, docLength));

  let beforeFrom = doc.lineAt(start).from;
  for (let n = doc.lineAt(start).number - 1; n >= 1; n -= 1) {
    const line = doc.line(n);
    if (isBlankLine(line.text)) {
      break;
    }
    beforeFrom = line.from;
    if (start - beforeFrom > CONTEXT_SLICE_MAX_CHARS) {
      break;
    }
  }

  let afterTo = doc.lineAt(end).to;
  for (let n = doc.lineAt(end).number + 1; n <= doc.lines; n += 1) {
    const line = doc.line(n);
    if (isBlankLine(line.text)) {
      break;
    }
    afterTo = line.to;
    if (afterTo - end > CONTEXT_SLICE_MAX_CHARS) {
      break;
    }
  }

  const before = doc.sliceString(beforeFrom, start).trim();
  const after = doc.sliceString(end, afterTo).trim();
  return {
    before: before.length > CONTEXT_SLICE_MAX_CHARS ? before.slice(-CONTEXT_SLICE_MAX_CHARS) : before,
    after: after.length > CONTEXT_SLICE_MAX_CHARS ? after.slice(0, CONTEXT_SLICE_MAX_CHARS) : after,
  };
}

/** 光标前文（续写输入）：取离光标最近的一段。 */
export function textBeforeCursor(doc: Text, pos: number): string {
  const end = Math.max(0, Math.min(pos, doc.length));
  return doc.sliceString(Math.max(0, end - CURSOR_SLICE_MAX_CHARS), end);
}
