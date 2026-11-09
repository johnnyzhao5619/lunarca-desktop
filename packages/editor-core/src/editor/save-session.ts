// 编辑器的保存路径（PRD-21-020：网页保存出的文件与桌面逐字节一致）。编辑器交给宿主写盘的正文只从
// `editorDocumentText` 出来；首次建档期间的编辑映射与保存串行化也在这里。写盘本身归宿主，且宿主原样落盘
// （桌面 `Vault::save_note` 不改行尾、不补末尾换行、不动 frontmatter）。

import { ChangeSet, Text, type EditorState } from "@codemirror/state";

/**
 * 编辑器交给宿主写盘的正文——**唯一出口**。就是 CM6 文档原样转成字符串，不做任何规范化：
 * - 行尾一律 `\n`：CM 装载时把 `\r\n` / `\r` / `\n` 都当换行拆成行，转回字符串时以 `\n` 相接。这条依赖编辑器
 *   **不设** `EditorState.lineSeparator`（设了就只认那一种换行，`\r` 会留在行里）。
 * - 末尾换行、frontmatter、空白一个字节都不补、不删：文档里是什么就写什么。
 */
export function editorDocumentText(state: EditorState): string {
  return state.doc.toString();
}

/** `original` → `next` 的最小改动：只换首尾之间真正不同的那一段（光标、装饰与滚动少受牵连）。 */
export function minimalChange(original: string, next: string): ChangeSet {
  let from = 0;
  while (from < original.length && from < next.length && original[from] === next[from]) from++;
  let tail = 0;
  while (tail < original.length - from && tail < next.length - from &&
    original[original.length - 1 - tail] === next[next.length - 1 - tail]) tail++;
  return ChangeSet.of({ from, to: original.length - tail, insert: next.slice(from, next.length - tail) }, original.length);
}

/** 将首次建档期间的 CM6 变更映射到后端返回的完整文件，保留建档元数据。 */
export function rebaseCreatedDraft(original: string, saved: string, edits: ChangeSet): string {
  return edits.map(minimalChange(original, saved)).apply(Text.of(saved.split("\n"))).toString();
}

/**
 * 正文在外面从 `original` 变成了 `next`（别处的版本、合并结果），而编辑器里自 `original` 以来又有了
 * `edits`（保存在途时接着打的字）：返回该应用到**此刻**文档上的那一次改动——别处的改动落下，
 * 这边接着打的字一个不丢。
 */
export function changeOverEdits(original: string, next: string, edits: ChangeSet): ChangeSet {
  return minimalChange(original, next).map(edits);
}

/** 同一编辑器的保存串行化；关闭/换页等待最新内容，而非较早一次保存。 */
export function serializedSave(save: () => Promise<boolean>, isDirty: () => boolean): () => Promise<boolean> {
  let pending: Promise<boolean> | null = null;
  return () => {
    if (pending) return pending;
    pending = (async () => {
      do {
        if (!(await save())) return false;
      } while (isDirty());
      return true;
    })().finally(() => { pending = null; });
    return pending;
  };
}
