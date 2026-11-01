/// <reference types="vite/client" />
import { describe, expect, it } from "vitest";
import { runtimeStyleCss as cssRaw } from "./style-source";

/**
 * 文本选择边界守护（PRD-07-060）。
 *
 * 契约：**界面外壳一律不可选，内容表面显式放开**。这件事很容易被后来的改动悄悄破坏——
 * 谁在某个组件上补一句 `user-select: text` 就多开一个口子，谁把根上那条规则挪走就整个失效，
 * 而 lint / typecheck / 视觉走查都发现不了（选不选得中要用鼠标划一下才知道）。
 *
 * 故本守护锁三件事：① 根上确有那条 `user-select: none`；② `user-select: text`
 * **只出现在唯一的白名单块里**（新增可选内容面必须加进那个块，而不是各写各的）；
 * ③ 白名单里确实包含编辑器与几处核心内容面（防止有人把块清空后测试仍绿）。
 */
describe("文本选择边界 · 聚合样式", () => {
  const stripped = cssRaw.replace(/\/\*[\s\S]*?\*\//g, (block: string) =>
    block.replace(/[^\n]/g, " "),
  );

  it("根元素声明 user-select: none（界面外壳默认不可选）", () => {
    // 负向断言前缀：`-webkit-user-select` 含有同样的子串，不加这道就会被它蒙混过关
    // （反向验证时正是这里先漏过的）。
    const rootRule = /(^|\})\s*html\s*\{[^}]*(?<!-webkit-)user-select:\s*none/m;
    expect(rootRule.test(stripped), "未找到根上的 `html { user-select: none }`").toBe(true);
  });

  it("放开选择只允许在唯一的白名单块里（不得各处零散开口子）", () => {
    // 每条 `user-select: text` 都必须落在同一个块内 → 出现次数 = 1（`-webkit-` 前缀另计）。
    const declarations = stripped.match(/(?<!-webkit-)user-select:\s*text/g) ?? [];
    expect(
      declarations.length,
      "`user-select: text` 出现了不止一处：新增可选内容面请加进白名单块的选择器列表，" +
        "而不是在自己的组件块里再写一条（否则边界会被逐步蚕食）",
    ).toBe(1);
  });

  it("白名单覆盖编辑器与核心内容面（含表单控件）", () => {
    const block = /([^{}]*)\{[^}]*(?<!-webkit-)user-select:\s*text/.exec(stripped);
    const selectors = block?.[1] ?? "";
    for (const required of [
      ".cm-content", // 笔记正文（contenteditable，漏了连编辑都会出问题）
      ".luna-markdown", // 阅读模式 / 对话回答 / 回收站预览共用的渲染基座
      ".luna-chat-message",
      ".luna-playback__cue-text", // 转写字幕
      ".luna-subtitle__source", // 实时字幕
      "input",
      "textarea",
    ]) {
      expect(selectors, `白名单缺少内容面 ${required}`).toContain(required);
    }
  });

  it("选中高亮走语义 Token（全应用一种选区色，含编辑器）", () => {
    expect(/::selection\s*\{\s*background:\s*var\(--color-selection\)/.test(stripped)).toBe(true);
    expect(stripped).not.toContain("--editor-selection");
  });
});
