import { describe, expect, it } from "vitest";
import { pulseState, pulseTopicSuggestion } from "./knowledge-pulse";
import type { KnowledgePulseDto, TopicSuggestionDto } from "../../ipc/bindings";

const DISTILL = { pending: 0, fresh: 0, edited: 0, empty: 0, running: false };

/** 一次脉搏取数（Rust `knowledge_pulse` 的形状）。 */
function pulse(
  patch: Partial<KnowledgePulseDto["distill"]> = {},
  rest: Partial<KnowledgePulseDto> = {},
): KnowledgePulseDto {
  return { distill: { ...DISTILL, ...patch }, topics: [], ...rest };
}

/** 知识库脉搏的基础三态（PRD-07-057 二次修订后续）。 */
describe("pulseState", () => {
  it("有待整理即行动态（edited 随行见证「绝不覆盖」承诺）", () => {
    expect(pulseState(pulse({ pending: 3, edited: 1 }))).toEqual({
      kind: "pending",
      pending: 3,
      edited: 1,
    });
  });

  it("无待整理但有沉淀 → 安静的正向态", () => {
    expect(pulseState(pulse({ fresh: 5, edited: 2 }))).toEqual({
      kind: "settled",
      fresh: 5,
      edited: 2,
    });
  });

  it("一无所有 → 采集引导；空正文资料不参与判定（既不可整理也不算沉淀）", () => {
    expect(pulseState(pulse())).toEqual({ kind: "empty" });
    expect(pulseState(pulse({ empty: 4 }))).toEqual({ kind: "empty" });
  });
});

/** 优先级栈：任何输入组合都只渲染栈顶一态（PRD-16-027 下一步引擎）。 */
describe("pulseState 的优先级栈", () => {
  const topic: TopicSuggestionDto = {
    suggestion_token: "topic-v1",
    tag: "项目X",
    notes: ["a.md", "b.md", "c.md"],
    note_count: 3,
    draft_items: null,
  };

  it("待整理压过其余一切——先把资料变成知识，才谈得上建页", () => {
    expect(pulseState(pulse({ pending: 2 }, { topics: [topic] })).kind).toBe("pending");
  });

  it("没有待整理才轮到主题页建议，并如实报还有几条", () => {
    const state = pulseState(
      pulse(
        { fresh: 5 },
        { topics: [topic, { suggestion_token: "topic-v2", tag: "另一个", notes: [], note_count: 0, draft_items: null }] },
      ),
    );
    expect(state).toEqual({ kind: "topic", suggestion: topic, more: 1 });
  });

  it("无建议时回落到安静的正向态，空库仍是空态", () => {
    expect(pulseState(pulse({ fresh: 5 })).kind).toBe("settled");
    expect(pulseState(pulse()).kind).toBe("empty");
  });
});

/** 主题页建议不再被栈顶饿死（2026-07-29 真机第一轮）。 */
describe("pulseTopicSuggestion", () => {
  const suggestion: TopicSuggestionDto = {
    suggestion_token: "topic-v1",
    tag: "交易",
    notes: ["a.md", "b.md", "c.md"],
    note_count: 3,
    draft_items: null,
  };

  it("栈顶是「待整理」时照样把建议摆出来", () => {
    // 真机现场：库里恒有待整理资料，于是 PRD-16-016 的 aha 时刻永远轮不到——
    // 这条守护钉死"不动栈、但给它并列的一行"。
    const data = pulse({ pending: 5 }, { topics: [suggestion] });
    const top = pulseState(data);
    expect(top.kind).toBe("pending");
    expect(pulseTopicSuggestion(data, top)).toEqual(suggestion);
  });

  it("建议已经是栈顶那一态时不重复呈现", () => {
    const data = pulse({}, { topics: [suggestion] });
    const top = pulseState(data);
    expect(top.kind).toBe("topic");
    expect(pulseTopicSuggestion(data, top)).toBeNull();
  });

  it("没有建议就整行不渲染", () => {
    const data = pulse({ pending: 2 });
    expect(pulseTopicSuggestion(data, pulseState(data))).toBeNull();
  });
});
