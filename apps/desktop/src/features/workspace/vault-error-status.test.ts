/// <reference types="vite/client" />
/**
 * 文件树、对象菜单、文件夹清单把 Vault 错误交给状态栏前必须经 `vaultErrorText`。
 *
 * 2026-10-05 真机：改名预检拒写时状态栏显示的是「VAULT_NOT_WRITABLE: 只读/引用.md」——
 * 这几处一直把 Rust 的「码: 载荷」原样上屏，只有编辑器标题那条路翻译过。
 */
import { describe, expect, it } from "vitest";

const SOURCES = import.meta.glob(
  ["./NoteTree.tsx", "./note-menu.tsx", "./use-note-folders.ts"],
  { eager: true, import: "default", query: "?raw" },
) as Record<string, string>;

const RAW_ERROR =
  /on(?:Status|Error)\(\s*(?:result\.error|failed\[0\]\.error)\s*\)|reason:\s*(?:result\.data\.)?failed\[0\]\.error\b/;

function rawErrorSites(sources: Record<string, string>): string[] {
  return Object.entries(sources)
    .filter(([, source]) => RAW_ERROR.test(source))
    .map(([path]) => path)
    .sort();
}

describe("Vault 错误上状态栏前经 vaultErrorText", () => {
  it("三个文件都扫到了，且没有原样上屏", () => {
    expect(Object.keys(SOURCES)).toHaveLength(3);
    expect(rawErrorSites(SOURCES)).toEqual([]);
  });

  it("反向夹具：原样交出去的会被抓出来", () => {
    expect(
      rawErrorSites({
        "a.tsx": "onStatus(result.error);",
        "b.tsx": "options.onError(failed[0].error);",
        "c.tsx": "t('x', { reason: result.data.failed[0].error })",
        "d.tsx": "onStatus(vaultErrorText(result.error));",
      }),
    ).toEqual(["a.tsx", "b.tsx", "c.tsx"]);
  });
});
