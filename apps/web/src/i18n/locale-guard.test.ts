/// <reference types="vite/client" />
/**
 * 网页文案守护（TDD-15 §4）：中英键集一致（取法与桌面同一份 `@lunarca/lint-config/locale-keys`），
 * 组件里写的静态 `t("…")` 键在语言文件里真的存在（漏写时两边同样地缺，键集守护看不出来）。
 */

import { describe, expect, it } from "vitest";
import { comparableLocaleKeys, flattenLocaleKeys, type LocaleDict } from "@lunarca/lint-config/locale-keys";
import { EDITOR_CORE_LOCALES, EDITOR_CORE_NAMESPACE } from "@lunarca/editor-core/i18n";

const LOCALES = import.meta.glob("./locales/*/*.json", { eager: true }) as Record<string, { default: LocaleDict }>;
const SOURCES = import.meta.glob("../**/*.tsx", { eager: true, import: "default", query: "?raw" }) as Record<string, string>;

function bundle(locale: string): LocaleDict {
  const entry = Object.entries(LOCALES).find(([path]) => path === `./locales/${locale}/web.json`);
  if (!entry) throw new Error(`缺 ${locale}/web.json`);
  return entry[1].default;
}

describe("网页文案", () => {
  const zh = bundle("zh-CN");
  const en = bundle("en-US");

  it("只有 web 一个命名空间，两种语言的文件清单一致", () => {
    const files = Object.keys(LOCALES).map((path) => path.replace(/^\.\/locales\/[^/]+\//, "")).sort();
    expect(new Set(files)).toEqual(new Set(["web.json"]));
    expect(Object.keys(LOCALES)).toHaveLength(2);
  });

  it("中英键集递归一致", () => {
    expect(comparableLocaleKeys(en)).toEqual(comparableLocaleKeys(zh));
  });

  it("组件里的静态 t(\"…\") 键都在语言文件里（带 `editor-core:` 前缀的查共享包自带的文案）", () => {
    const keys = new Set(flattenLocaleKeys(zh));
    const coreKeys = new Set(flattenLocaleKeys(EDITOR_CORE_LOCALES["zh-CN"] as LocaleDict));
    const missing: string[] = [];
    let prefixed = 0;
    for (const [path, source] of Object.entries(SOURCES)) {
      if (path.includes(".test.") || !source.includes('useTranslation("web")')) continue;
      for (const match of source.matchAll(/\bt\(\s*"([^"$]+)"/g)) {
        const [ns, key] = match[1].includes(":") ? match[1].split(":", 2) : ["web", match[1]];
        if (ns === EDITOR_CORE_NAMESPACE) prefixed += 1;
        const known = ns === "web" ? keys : ns === EDITOR_CORE_NAMESPACE ? coreKeys : null;
        if (!known?.has(key)) missing.push(`${path}: ${match[1]}`);
      }
    }
    expect(missing).toEqual([]);
    expect(prefixed).toBeGreaterThan(0);
  });

  it("Worker 写回命令可能回的每个稳定码都有中英文案（不认得的码只会显示通用一句加码本身）", () => {
    const codes = [
      "network_unavailable",
      "sync_quota_exceeded",
      "sync_read_only",
      "account_frozen",
      "too_large",
      "too_large_unknown",
      "path_taken",
      "invalid_name_empty",
      "invalid_name_reserved",
      "invalid_name_separator",
      "invalid_name_hidden",
      "not_found",
      "not_text",
      "chunk_missing",
      "content_mismatch",
      "sync_remote_corrupt",
      "conflict_copy_exhausted",
      "vault_create_exhausted",
      "sync_busy",
      "vault_not_open",
    ];
    for (const bundleOf of [zh, en]) {
      const keys = new Set(flattenLocaleKeys(bundleOf));
      expect(codes.filter((code) => !keys.has(`error.${code}`))).toEqual([]);
    }
    // 只读横幅：三种写入闸各有一句。
    for (const gate of ["sync_quota_exceeded", "sync_read_only", "account_frozen"]) {
      expect(flattenLocaleKeys(zh)).toContain(`note.readOnly.${gate}`);
    }
  });

  it("英文那一份不冒中文", () => {
    const chinese = flattenLocaleKeys(en).filter((key) => {
      const value = key.split(".").reduce<unknown>((node, part) => (node as LocaleDict)[part], en);
      return typeof value === "string" && /[一-鿿]/.test(value);
    });
    expect(chinese).toEqual([]);
  });
});
