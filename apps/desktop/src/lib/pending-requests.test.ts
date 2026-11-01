import { describe, expect, it } from "vitest";
import {
  consumePendingEditorReveal,
  consumePendingVersionHistory,
  requestEditorReveal,
  requestVersionHistory,
} from "./app-events";

/**
 * 「查看变更」的暂存兑现（PRD-16-025 / 批次 3 验收轮缺陷的机器防线）。
 *
 * 为什么必须有暂存：`dispatchEvent` 是同步的，而「打开那篇笔记」要等 React 重渲染——
 * 目标页尚未打开时（自动归入的主题页几乎必然如此），事件在监听器按 `pathRef` 匹配的
 * 那一刻派发进真空。只靠事件的话，「查看变更」点了只开笔记、版本历史面板永远不弹。
 */
describe("版本历史请求的暂存与兑现", () => {
  it("请求先暂存，目标编辑器加载完成时一次性兑现", () => {
    requestVersionHistory("Notes/项目 X.md", 1_000);
    // 别的路径的编辑器认领不走它。
    expect(consumePendingVersionHistory("Notes/别的.md", 1_100)).toBe(false);
    // 目标路径命中即消费。
    expect(consumePendingVersionHistory("Notes/项目 X.md", 1_200)).toBe(true);
    // 一次性：多 pane 打开同一篇时只有先加载完的那个弹面板，第二个不重复弹。
    expect(consumePendingVersionHistory("Notes/项目 X.md", 1_300)).toBe(false);
  });

  it("过期的暂存不兑现——旧请求不该在几分钟后用户自己点开那篇时突然弹面板", () => {
    requestVersionHistory("Notes/项目 X.md", 1_000);
    expect(consumePendingVersionHistory("Notes/项目 X.md", 1_000 + 60_000)).toBe(false);
    // 过期即清空，不残留。
    expect(consumePendingVersionHistory("Notes/项目 X.md", 1_100)).toBe(false);
  });

  it("后一条请求顶替前一条（用户连点两篇的「查看变更」，只兑现最后那篇）", () => {
    requestVersionHistory("Notes/甲.md", 1_000);
    requestVersionHistory("Notes/乙.md", 1_100);
    expect(consumePendingVersionHistory("Notes/甲.md", 1_200)).toBe(false);
    expect(consumePendingVersionHistory("Notes/乙.md", 1_300)).toBe(true);
  });
});

/**
 * 编辑器定位（reveal）的暂存兑现——与版本历史**同一竞态、同一机制**（L2 遗留）。
 *
 * 此前 reveal 只有实例级暂存（路径已匹配但内容未加载），盖不住「这篇根本还没打开」：
 * 监听器在事件派发那一刻持着旧 pathRef 直接丢弃，症状是"搜索点命中只打开笔记、不跳行"。
 */
describe("编辑器定位请求的暂存与兑现", () => {
  it("目标未打开时靠暂存接住，加载完成兑现完整 detail（含行号）", () => {
    requestEditorReveal({ path: "Notes/甲.md", line: 42, headingIndex: null, focusEditor: true }, 1_000);
    expect(consumePendingEditorReveal("Notes/乙.md", 1_100)).toBeNull();
    expect(consumePendingEditorReveal("Notes/甲.md", 1_200)).toEqual({
      path: "Notes/甲.md",
      line: 42,
      headingIndex: null,
      focusEditor: true,
    });
    // 一次性：第二个 pane 加载同一篇不重复跳行。
    expect(consumePendingEditorReveal("Notes/甲.md", 1_300)).toBeNull();
  });

  it("过期不兑现——旧的跳行请求不该在几分钟后用户自己点开那篇时突然生效", () => {
    requestEditorReveal({ path: "Notes/甲.md", line: 7, headingIndex: null, focusEditor: true }, 1_000);
    expect(consumePendingEditorReveal("Notes/甲.md", 1_000 + 60_000)).toBeNull();
  });

  it("两类请求各自独立暂存，互不顶替", () => {
    requestVersionHistory("Notes/甲.md", 1_000);
    requestEditorReveal({ path: "Notes/乙.md", line: 3, headingIndex: null, focusEditor: true }, 1_000);
    expect(consumePendingVersionHistory("Notes/甲.md", 1_100)).toBe(true);
    expect(consumePendingEditorReveal("Notes/乙.md", 1_100)).not.toBeNull();
  });
});
