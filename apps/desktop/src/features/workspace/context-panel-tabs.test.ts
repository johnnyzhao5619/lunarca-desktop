/**
 * 「上下文」面板页签集守护（PRD-07-031 / PRD-07-063 / PRD-07-065）。
 *
 * 三条断言各拦一种真实处境：
 *
 * 1. **页签文案缺 key** —— i18next 缺 key 时回落成裸串，用户在面板上读到
 *    `panel.noteTags` 这种东西。
 * 2. **字数被搬进「笔记信息」** —— PRD-07-065 是真机上「同一件事同屏出现三次、字数
 *    两次且措辞还不一样」修出来的，分工写死为「状态栏 = 量化事实」。把字数放进右侧面板
 *    会当场复现那个缺陷，而且更坏：状态栏那个随输入实时变化，面板这个是落盘事实，
 *    两个数必然有一刻不相等。
 * 3. **类型译名被另抄一份** —— 图谱侧栏与本面板说的是同一句话；抄出来的那一份会在
 *    改名那天分叉，而没有任何东西会因此变红。
 */

import { describe, expect, it } from "vitest";

import zhWorkspace from "../../i18n/locales/zh-CN/workspace.json";
import enWorkspace from "../../i18n/locales/en-US/workspace.json";
import { NOTE_TYPES, noteTypeLabelKey } from "../../lib/note-type";
import { CONTEXT_TABS } from "./context-panel-tabs";

type Dict = Record<string, unknown>;

function lookup(bundle: Dict, key: string): unknown {
  return key
    .split(".")
    .reduce<unknown>((node, seg) => (node as Dict | undefined)?.[seg], bundle);
}

const BUNDLES = [
  ["zh", zhWorkspace as unknown as Dict],
  ["en", enWorkspace as unknown as Dict],
] as const;

describe("上下文面板页签集", () => {
  it("两个对象级页签在场，且排在关系类之后、版本之前", () => {
    const ids = CONTEXT_TABS.map((tab) => tab.id);
    expect(ids).toEqual([
      "outline",
      "backlinks",
      "outgoing",
      "related",
      "tags",
      "info",
      "history",
    ]);
    // 版本历史谈的是「这一篇的过去」，与「这一篇是什么」不是一档——它必须留在末位。
    expect(ids[ids.length - 1]).toBe("history");
  });

  it("每个页签的文案在两份资源包里都在（缺 key 会渲染成裸串）", () => {
    for (const tab of CONTEXT_TABS) {
      for (const [name, bundle] of BUNDLES) {
        expect(lookup(bundle, tab.label), `${name} 缺 ${tab.label}`).toBeTruthy();
      }
    }
  });

  /**
   * 字数只在状态栏说一次（PRD-07-065）。
   *
   * 判据落在**资源包**上而不是组件源码：文案是它能被搬进面板的唯一途径——要在面板里显示
   * 字数，就得先有一条 `panel.*` 的字数文案。反过来扫组件源码找 "wordCount" 会被一句
   * 注释绕过（而注释正是本仓其它门禁踩过的坑）。
   */
  it("「笔记信息」不含字数——量化事实归状态栏", () => {
    for (const [name, bundle] of BUNDLES) {
      const panel = (bundle.panel ?? {}) as Dict;
      const wordish = Object.keys(panel).filter((key) => /word|count|字数/i.test(key));
      expect(wordish, `${name} 的 panel.* 里出现了字数文案：${wordish.join(", ")}`).toEqual([]);
      // 状态栏那一条仍在——两半合起来才是「只说一次」，缺了后半句这条断言会在字数
      // 被整个删掉时照样绿。
      expect(lookup(bundle, "statusbar.words"), `${name} 缺 statusbar.words`).toBeTruthy();
    }
  });

  /**
   * 类型译名走中性 helper，两份资源包都得有。
   *
   * **不从 `features/graph` 引注册表来比对**：那会让 `workspace` 依赖 `graph`，而
   * `graph` 已经依赖 `workspace`——结构门禁会当场报「功能模块存在循环依赖」（本批第一版
   * 就是这么被拦住的）。图谱那份注册表用的是不是同一个 helper，由**图谱自己的**守护
   * 断言（`graph-rescan-i18n.test.ts`）——判据落在有依赖的那一侧，不制造反向依赖。
   */
  it("类型译名走中性 helper，两份资源包都在", () => {
    for (const noteType of NOTE_TYPES) {
      const key = noteTypeLabelKey(noteType);
      for (const [name, bundle] of BUNDLES) {
        expect(lookup(bundle, key), `${name} 缺 ${key}`).toBeTruthy();
      }
    }
  });
});
