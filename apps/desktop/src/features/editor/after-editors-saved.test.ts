/// <reference types="vite/client" />
/**
 * 改名 / 移动先过保存屏障（TDD-07 §2.4、PRD-06-030 ②）：改名重构会改写别的笔记，
 * 那些笔记里未落盘的输入必须先落盘，否则改写器读到旧版、或改写后撞基线冲突。
 */
import { describe, expect, it } from "vitest";
import { afterEditorsSaved, registerEditorSave } from "./editor-save-guard";

const SOURCES = import.meta.glob(["../../**/*.{ts,tsx}", "!../../**/*.test.{ts,tsx}"], {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/** 会改写别的笔记的命令：改名、移动、速记转正 / 撤销、撤销改名。 */
const RELINKING = /commands\.(vaultRename|vaultMoveInto|promoteQuickNote|demoteToQuickNote|vaultUndoRelocation|repairExternalRenames)\(/g;
const GUARDED =
  /afterEditorsSaved\(\s*\(\)\s*=>\s*commands\.(vaultRename|vaultMoveInto|promoteQuickNote|demoteToQuickNote|vaultUndoRelocation|repairExternalRenames)\(/g;

function unguardedCalls(sources: Record<string, string>): string[] {
  return Object.entries(sources)
    .filter(([, source]) => (source.match(RELINKING)?.length ?? 0) > (source.match(GUARDED)?.length ?? 0))
    .map(([path]) => path)
    .sort();
}

describe("afterEditorsSaved", () => {
  it("有一篇落不了盘：不执行，交回 blocked 的结果", async () => {
    const dispose = registerEditorSave(async () => false);
    let ran = false;
    const result = await afterEditorsSaved(
      async () => {
        ran = true;
        return "done";
      },
      () => "blocked",
    );
    dispose();
    expect(ran).toBe(false);
    expect(result).toBe("blocked");
  });

  it("全部落盘：执行并原样返回结果", async () => {
    const order: string[] = [];
    const dispose = registerEditorSave(async () => {
      order.push("save");
      return true;
    });
    const result = await afterEditorsSaved(
      async () => {
        order.push("rename");
        return 42;
      },
      () => -1,
    );
    dispose();
    expect(result).toBe(42);
    expect(order).toEqual(["save", "rename"]);
  });
});

describe("会改写别的笔记的命令都先过保存屏障", () => {
  it("全部源码：没有直接调用", () => {
    expect(unguardedCalls(SOURCES)).toEqual([]);
  });

  it("守护确实扫到了调用点（空跑会让上一条恒绿）", () => {
    const total = Object.values(SOURCES).reduce((n, s) => n + (s.match(GUARDED)?.length ?? 0), 0);
    expect(total).toBeGreaterThanOrEqual(8);
  });

  it("反向夹具：直接调用会被抓出来", () => {
    expect(
      unguardedCalls({
        "a.tsx": "await commands.vaultRename(path, name);",
        "b.tsx": "afterEditorsSaved(() => commands.vaultMoveInto(p, d), blocked); commands.vaultRename(x, y);",
        "c.tsx": "afterEditorsSaved(\n  () => commands.vaultRename(p, n),\n  blocked,\n);",
      }),
    ).toEqual(["a.tsx", "b.tsx"]);
  });
});
