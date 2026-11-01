import { describe, expect, it } from "vitest";

/**
 * **静态 `t("…")` 键必须真的存在**——本仓被这一类漏法咬过三次。
 *
 * 中英 parity 守护（`locale-parity.test.ts`）只保证**两边一样**，而漏写文案时两边
 * **同样地缺**，于是 typecheck / lint / parity 全绿，用户看到的是一整块裸 key：
 * 批次 4 的语义索引面板整块显示 `semantic.indexed` / `semantic.build`（2026-07-29 真机
 * 第一轮撞上）、批次 2/3/4 的用量 origin 标签也是同一种漏法。
 *
 * 判据只能是「**组件里写的 key** ↔ **语言文件里有的 key**」，故这条守护同时读源码与
 * 真实语言文件：
 * - namespace 由同文件的 `useTranslation("<ns>")` 决定；带 `ns:` 前缀的 key 以前缀为准；
 * - 只查**字面量** `t("…")`——模板串 `t(\`a.${x}\`)` 与变量 `t(key)` 是动态查表，
 *   它们各自有专属守护（图谱类型过滤器、体检卡、队列阶段…），静态扫描看不到也不该假装看得到。
 */

const SOURCES = import.meta.glob("../features/**/*.tsx", {
  eager: true,
  query: "?raw",
  import: "default",
}) as Record<string, string>;

const LOCALES = import.meta.glob("./locales/*/*.json", { eager: true }) as Record<
  string,
  { default: Record<string, unknown> }
>;

/** locale → namespace → 扁平键集。 */
function bundles(): Map<string, Map<string, Set<string>>> {
  const out = new Map<string, Map<string, Set<string>>>();
  const flatten = (node: Record<string, unknown>, prefix: string): string[] =>
    Object.entries(node).flatMap(([key, value]) =>
      typeof value === "object" && value !== null
        ? flatten(value as Record<string, unknown>, `${prefix}${key}.`)
        : [`${prefix}${key}`],
    );
  for (const [path, mod] of Object.entries(LOCALES)) {
    const match = /\.\/locales\/([^/]+)\/([^/]+)\.json$/.exec(path);
    if (!match) {
      continue;
    }
    const [, locale, ns] = match;
    if (!out.has(locale!)) {
      out.set(locale!, new Map());
    }
    out.get(locale!)!.set(ns!, new Set(flatten(mod.default, "")));
  }
  return out;
}

/** 一处静态用法：哪个文件、哪个 namespace、哪个 key。 */
interface Usage {
  file: string;
  ns: string;
  key: string;
}

function collectUsages(): Usage[] {
  const out: Usage[] = [];
  for (const [file, source] of Object.entries(SOURCES)) {
    // 一个组件可能 `useTranslation("workspace")`，也可能不用（纯展示组件）。
    const nsMatch = /useTranslation\(\s*"([a-zA-Z0-9_-]+)"/.exec(source);
    const defaultNs = nsMatch?.[1];
    for (const [, raw] of source.matchAll(/\bt\(\s*"([^"`$]+)"/g)) {
      const key = raw!;
      const [maybeNs, ...rest] = key.split(":");
      if (rest.length > 0) {
        out.push({ file, ns: maybeNs!, key: rest.join(":") });
      } else if (defaultNs) {
        out.push({ file, ns: defaultNs, key });
      }
      // 没有 namespace 又没前缀 → 无从判定，跳过（不猜）。
    }
  }
  return out;
}

describe("组件里写死的 i18n key 必须在两个语言文件里都存在", () => {
  const locales = bundles();
  const usages = collectUsages();

  it("扫到了足够多的用法（守护自身不能空转）", () => {
    // 静态用法在本仓是几百条量级；扫不到说明 glob 或正则坏了，而那会让这条守护
    // 变成一个永远绿的摆设。
    expect(usages.length).toBeGreaterThan(200);
  });

  it("每一处静态 key 都能查到文案", () => {
    const missing: string[] = [];
    for (const [locale, byNs] of locales) {
      for (const usage of usages) {
        const keys = byNs.get(usage.ns);
        if (!keys) {
          missing.push(`${locale}: 找不到 namespace ${usage.ns}（${usage.file}）`);
          continue;
        }
        if (!keys.has(usage.key)) {
          missing.push(`${locale}/${usage.ns}.json 缺 ${usage.key}（${usage.file}）`);
        }
      }
    }
    expect(missing, `共 ${missing.length} 处裸 key`).toEqual([]);
  });
});
