/// <reference types="vite/client" />
/**
 * 「库变了就重取」的**唯一实现**守护（TDD-07 §4.2d）。
 *
 * 判据只有一条，机器读得出：**不看 `rel_path` 的 `vault://changed` 订阅一律走
 * `useVaultReload`**。看 `rel_path` 的（回放、蒸馏徽标、词典面板、编辑器的外部改动同步）
 * 是另一件事——它们问的是"变的是不是我这一篇"，照常自己订阅。
 *
 * 这条规则是被违反出来的：`vault://changed` 有 12 个不看路径的消费者，其中 **3 个**
 * 各自抄了一份 `setTimeout` 去抖（连常量名都叫 `RELOAD_DEBOUNCE_MS`），另外 9 个一份也
 * 没有。于是一次批量移动逐条广播时，后者每条各拉一遍全量数据（整棵树、整张图、整份
 * 清单），全部串在 Vault 锁上——2026-09-02 真机「拖完之后界面不动」的前端一半。
 * 少一份去抖不会有任何东西变红，只会让某一次操作莫名其妙地卡住。
 */

import { describe, expect, it } from "vitest";

const SOURCES = import.meta.glob("../{app,features,components,hooks,lib}/**/*.{ts,tsx}", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/** 共享实现自己当然订阅（它就是那一份）。glob 的键相对本文件所在目录。 */
const SHARED_HOOK = "./use-vault-reload.ts";

const VIEWS = Object.fromEntries(
  Object.entries(SOURCES).filter(([path]) => !path.includes(".test.") && path !== SHARED_HOOK),
);

/**
 * 忽略事件载荷的订阅：`listen(() => …)`、`listen(load)`、`listen(async () => …)`。
 * 收下参数的（`listen((event) => …)`）不算——那是按路径过滤，本规则不管它。
 */
export function payloadBlindSubscriptions(source: string): string[] {
  return [...source.matchAll(/vaultChanged\.listen\(\s*(async\s*)?(\(\s*\)|[A-Za-z_$][\w$]*\s*\))/g)].map(
    (match) => match[0],
  );
}

describe("库变了就重取（唯一实现）", () => {
  it("扫描面非空（glob 失效时本用例先红）", () => {
    expect(Object.keys(VIEWS).length).toBeGreaterThan(50);
    expect(SOURCES[SHARED_HOOK]).toContain("vaultChanged.listen");
  });

  it("不看 rel_path 的订阅全部走 useVaultReload", () => {
    const offenders = Object.entries(VIEWS).flatMap(([path, source]) =>
      payloadBlindSubscriptions(source).map((hit) => `${path}: ${hit}`),
    );
    expect(
      offenders,
      `不看路径的重取请用 useVaultReload（同一阵变更合并成一次，见 hooks/use-vault-reload.ts）：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("也没有第二份手抄的去抖常量", () => {
    const offenders = Object.entries(VIEWS)
      .filter(([, source]) => source.includes("RELOAD_DEBOUNCE_MS"))
      .map(([path]) => path);
    expect(
      offenders,
      `合并窗口只有一处（use-vault-reload.ts 里那一个档位）：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("反向验证：判据抓得住违规、放得过按路径过滤的订阅", () => {
    expect(payloadBlindSubscriptions("events.vaultChanged.listen(() => refresh())")).toHaveLength(1);
    expect(payloadBlindSubscriptions("events.vaultChanged.listen(load)")).toHaveLength(1);
    expect(payloadBlindSubscriptions("events.vaultChanged.listen(async () => refresh())")).toHaveLength(1);
    // 收下 event 的是"变的是不是我这一篇"，不归本规则管。
    expect(payloadBlindSubscriptions("events.vaultChanged.listen((event) => f(event))")).toEqual([]);
  });
});
