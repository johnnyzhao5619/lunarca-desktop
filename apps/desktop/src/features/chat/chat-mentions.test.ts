import { describe, expect, it } from "vitest";
import { activeMention, applyMention } from "./chat-mentions";

describe("@ 提及的文本层判定（PRD-16-009）", () => {
  it("刚敲下 @ 就该弹候选，打字即收窄", () => {
    expect(activeMention("@", 1)).toEqual({ start: 0, end: 1, query: "" });
    expect(activeMention("上周 @工作", 6)).toEqual({ start: 3, end: 6, query: "工作" });
  });

  it("邮箱与词中 @ 不是提及", () => {
    // `a@b.com` 被当成提及会在每一次贴邮箱时弹出候选面板。
    expect(activeMention("me@example.com", 14)).toBeNull();
    expect(activeMention("项目X@2", 5)).toBeNull();
  });

  it("CJK/标点前导的 @ 是提及——中文输入不打空格是常态", () => {
    // email 的 local part 只能是 ASCII，故 CJK 前导不可能是邮箱；按"非空白就挡"
    // 会让 @ 在中文句子中间彻底失灵（「关于@工作」弹不出候选，验收轮缺陷）。
    expect(activeMention("关于@工作", 5)).toEqual({ start: 2, end: 5, query: "工作" });
    expect(activeMention("（@项目", 4)).toEqual({ start: 1, end: 4, query: "项目" });
    expect(activeMention("看看，@纪要", 6)).toEqual({ start: 3, end: 6, query: "纪要" });
  });

  it("打了空格即结束——提及是一个词", () => {
    expect(activeMention("@工作 都聊了什么", 8)).toBeNull();
  });

  it("光标不在提及末尾时不算正在写它", () => {
    // 用户回头改前面的字：那一处不该突然弹出候选。
    expect(activeMention("@工作 都聊了什么", 2)).toEqual({ start: 0, end: 2, query: "工" });
    expect(activeMention("你好 @工作", 2)).toBeNull();
  });

  it("选中后把 @… 片段整个删掉，并且不把前后词粘在一起", () => {
    const text = "上周 @工作 的进展";
    const mention = activeMention("上周 @工作", 6)!;
    // 尾部已自带空格 → 不再补一个；光标停在被删掉那段的起点。
    expect(applyMention(text, mention)).toEqual({ text: "上周 的进展", caret: 2 });
    // 只有一个提及时不留多余空格。
    const only = activeMention("@工作", 3)!;
    expect(applyMention("@工作", only)).toEqual({ text: "", caret: 0 });
  });
});
