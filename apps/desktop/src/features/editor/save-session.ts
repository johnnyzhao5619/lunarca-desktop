import { ChangeSet, Text } from "@codemirror/state";

/** 将首次建档期间的 CM6 变更映射到后端返回的完整文件，保留建档元数据。 */
export function rebaseCreatedDraft(original: string, saved: string, edits: ChangeSet): string {
  let from = 0;
  while (from < original.length && from < saved.length && original[from] === saved[from]) from++;
  let tail = 0;
  while (tail < original.length - from && tail < saved.length - from &&
    original[original.length - 1 - tail] === saved[saved.length - 1 - tail]) tail++;
  const created = ChangeSet.of({ from, to: original.length - tail,
    insert: saved.slice(from, saved.length - tail) }, original.length);
  return edits.map(created).apply(Text.of(saved.split("\n"))).toString();
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
