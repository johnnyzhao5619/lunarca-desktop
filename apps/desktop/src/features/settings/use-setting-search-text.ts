/**
 * 搜索匹配用的本地化文案取值器（`SettingSearchText`）的**唯一构造处**。
 *
 * 侧栏、主区结果、以及主区那条「另有 N 个高级结果」各自构造过一份——三份逐字相同的
 * 对象字面量，而搜索面一变（本轮加了页名、分节名与同义词）就得记着改三处，漏掉哪处
 * 都表现为"侧栏说有命中、列表里却没有"这种没人能一眼看出的分叉。
 */

import { useMemo } from "react";
import { useTranslation } from "react-i18next";

import type { SettingSearchText } from "./settings-filter";

export function useSettingSearchText(): SettingSearchText {
  const { t } = useTranslation("settings");
  return useMemo(
    () => ({
      label: (spec) => t(spec.i18n_key),
      description: (spec) => t(spec.desc_i18n_key, { defaultValue: "" }),
      // 同义词与说明同一条派生约定（标签 key + 后缀），没声明就是空串。
      aliases: (spec) => t(`${spec.i18n_key}_aliases`, { defaultValue: "" }),
      pageLabel: (page) => t(`group.${page}`),
      // **只取策展过的别名，不取页说明**：说明是散文，把它放进匹配面等于"页说明里出现过
      // 的任何一个词都能把整页拉进结果"——搜「语言」会从 5 条涨到 19 条，因为好几页的
      // 说明里都写了「语言」。
      pageAliases: (page) => t(`group_aliases.${page}`, { defaultValue: "" }),
      sectionLabel: (spec) =>
        spec.section ? t(`section.${spec.group}.${spec.section}`, { defaultValue: "" }) : "",
    }),
    [t],
  );
}
