// 语言文件的**键集**取法（中英 parity 守护的共同底座）。
//
// 两处守护（`locale-parity.test.ts` 全 namespace、`graph-rescan-i18n.test.ts` 的
// workspace 文件）此前各写了一份 `flatten`。判据一样、写两份，迟早只有一份跟上变化——
// 而它们守的恰恰是"两边必须一样"这件事。

/** 任意深度的语言文件节点。 */
export type LocaleDict = Record<string, unknown>;

/**
 * i18next 的复数后缀。
 *
 * **复数形态是逐语言的语法，不是"漏译"**：英语要 `_one` / `_other` 两条，中文一条都
 * 不要。要求两边键集逐字相同，等于禁止在这个应用里正确处理英文单复数——「1 days」那种
 * 写法不该出现在一个商业产品里。故比较**基名**：`x_one` 与 `x` 视为同一个键。
 *
 * 这不是把门禁放松：少写一整个键（如英文缺了 `main.foo`）仍然当场判红，因为它的基名
 * 在另一边根本不存在。
 */
const PLURAL_SUFFIXES = ["_zero", "_one", "_two", "_few", "_many", "_other"] as const;

/** 去掉复数后缀，得到这条文案的**基名**。 */
export function pluralBaseKey(key: string): string {
  const suffix = PLURAL_SUFFIXES.find((candidate) => key.endsWith(candidate));
  return suffix ? key.slice(0, -suffix.length) : key;
}

/** 递归展平成 `a.b.c` 形式的键集。 */
export function flattenLocaleKeys(node: LocaleDict, prefix = ""): string[] {
  return Object.entries(node).flatMap(([key, value]) =>
    typeof value === "object" && value !== null
      ? flattenLocaleKeys(value as LocaleDict, `${prefix}${key}.`)
      : [`${prefix}${key}`],
  );
}

/** 用于中英比对的键集：展平 → 去复数后缀 → 去重 → 排序。 */
export function comparableLocaleKeys(node: LocaleDict): string[] {
  return [...new Set(flattenLocaleKeys(node).map(pluralBaseKey))].sort();
}
