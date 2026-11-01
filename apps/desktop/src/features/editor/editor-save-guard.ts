/** 同一窗口所有编辑面在移除 / 导航前共用的保存屏障；正文仍只在各 CM6 实例里。 */
const saves = new Map<symbol, { save: () => Promise<boolean>; hold?: (held: boolean) => void }>();
let exitPending = false;
let navigationPending = 0;
function updateHolds() {
  for (const entry of saves.values()) entry.hold?.(exitPending || navigationPending > 0);
}
export function registerEditorSave(save: () => Promise<boolean>, hold?: (held: boolean) => void): () => void {
  const id = Symbol(); saves.set(id, { save, hold }); hold?.(exitPending || navigationPending > 0);
  return () => { hold?.(false); saves.delete(id); };
}
export function holdEditorInputs(held: boolean) {
  exitPending = held;
  updateHolds();
}
export async function flushEditorSaves(): Promise<boolean> {
  for (const { save } of saves.values()) {
    try { if (!(await save())) return false; }
    catch { return false; }
  }
  return true;
}
export async function withEditorSaveGuard(action: () => void | Promise<void>): Promise<boolean> {
  if (exitPending) return false;
  navigationPending++; updateHolds();
  try {
    if (!(await flushEditorSaves()) || exitPending) return false;
    await action(); return true;
  } finally { navigationPending--; updateHolds(); }
}

/**
 * 改名 / 移动会改写**别的**笔记里的链接（TDD-07 §2.4、TDD-06 §3 改名重构）：先让所有编辑面
 * 落盘、冻结输入，再执行；有一篇落不了盘（外部冲突等）就不执行，交回 `blocked()` 给的结果。
 * 不先落盘的话，改写器读到的是旧版，或改写之后在用户手里撞上基线冲突。
 */
export async function afterEditorsSaved<T>(action: () => Promise<T>, blocked: () => T): Promise<T> {
  let out: { value: T } | null = null;
  const ran = await withEditorSaveGuard(async () => {
    out = { value: await action() };
  });
  const done = out as { value: T } | null;
  return ran && done ? done.value : blocked();
}
