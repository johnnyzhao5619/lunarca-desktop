import { describe, expect, it } from "vitest";

// 外部改动横幅的「合并两边改动」（ADR-148 ②、TDD-06 §4.2）。合并本身是 Rust 纯函数（luna-vault
// merge3，单测在那里）；VaultEditor 没有渲染级测试夹具，这里按源码锚定前端的四处接线：
// 基线取已保存内容、等待期间状态变了就不落、结果作为可撤销的编辑落下、合并不了就展开对比。

const editors = import.meta.glob("./VaultEditor.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/** 去掉注释再断言：注释里提到的名字不能替代真实接线。 */
function code(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
}

const EDITOR = code(Object.values(editors)[0]);
const MERGE = EDITOR.slice(
  EDITOR.indexOf("async function mergeExternalContent()"),
  EDITOR.indexOf("async function saveConflictCopy()"),
);

describe("外部改动横幅：合并两边改动", () => {
  it("扫描面非空", () => {
    expect(MERGE.length).toBeGreaterThan(200);
  });

  it("基线是已保存内容，我方是缓冲区，对方是磁盘版本", () => {
    expect(MERGE).toContain("commands.noteMerge(savedContentRef.current, local, external)");
  });

  it("等待期间横幅已处理、换了笔记、外部又改或用户又打字，结果都不落", () => {
    const guard = MERGE.match(/if \(!externalPendingRef\.current[^\n]*return;/)?.[0] ?? "";
    expect(guard).toContain("pathRef.current !== requestedPath");
    expect(guard).toContain("externalContentRef.current !== external");
    expect(guard).toContain("currentDocumentText() !== local");
  });

  it("结果作为一次可撤销的编辑落下，再以磁盘版本为基线走保存护栏", () => {
    expect(MERGE).toMatch(/view\.dispatch\(\{ changes: outcome\.change, userEvent: "input\.merge" \}\)/);
    expect(MERGE).not.toContain("applyIncrementalText");
    const dispatchAt = MERGE.indexOf("view.dispatch(");
    const baselineAt = MERGE.indexOf("savedContentRef.current = external;");
    const flushAt = MERGE.indexOf("flushRef.current()");
    expect(dispatchAt).toBeGreaterThan(-1);
    expect(baselineAt).toBeGreaterThan(dispatchAt);
    expect(flushAt).toBeGreaterThan(baselineAt);
  });

  it("两边改了同一处或改动太多：说清楚并展开对比，不改缓冲区", () => {
    const refused = MERGE.slice(MERGE.indexOf('if (outcome.kind !== "merged")'), MERGE.indexOf("view.dispatch("));
    expect(refused).toContain('"conflictMergeOverlap"');
    expect(refused).toContain('"conflictMergeTooLarge"');
    expect(refused).toContain("compareExternalContent()");
    expect(refused).toContain("return;");
  });

  it("横幅上有这个按钮，外部删除时不可点", () => {
    const button = EDITOR.match(/<button[^>]*onClick=\{\(\) => \{ void mergeExternalContent\(\); \}\}>/)?.[0] ?? "";
    expect(button).toContain("disabled={externalContentRef.current === null}");
  });
});
