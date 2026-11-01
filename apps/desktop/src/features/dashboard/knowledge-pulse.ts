/**
 * 知识库脉搏的态判定（PRD-16-027 下一步引擎）。纯逻辑，可 headless 测试。
 *
 * 弹窗要回答的问题是「我的知识库此刻什么状态、有什么值得做」——数据源是 Rust 一次下发的
 * `knowledge_pulse`（批量蒸馏预检四桶 + 主题页建议），判据与单篇徽标/建议条同源，
 * 不另造第二套盘点。
 */

import type { KnowledgePulseDto, TopicSuggestionDto } from "../../ipc/bindings";

export type KnowledgePulseState =
  /** 有资料待整理：脉搏的可行动核心，主按钮直达批量确认卡。 */
  | { kind: "pending"; pending: number; edited: number }
  /** 有笔记簇可合成主题页（PRD-16-016 aha 时刻）：知识复利循环的入口。
   *  呈现（含合成进行态与入队报错）整体归 `TopicSuggestionRow`，本态只携带事实。 */
  | { kind: "topic"; suggestion: TopicSuggestionDto; more: number }
  /** 资料都已整理：安静的正向态（fresh/edited 作小字见证）。 */
  | { kind: "settled"; fresh: number; edited: number }
  /** 还没有任何资料：引导去采集（流水线的起点）。 */
  | { kind: "empty" };

/**
 * 主题页建议**不进栈，另占并列的一行**。
 *
 * 它若排在栈里，PRD-16-016 的 aha 时刻（"这几篇能合成一页"）就结构性不可达：真实库里
 * "待整理"几乎恒不为零，用户在把资料整完之前一次都见不到建议。栈的顺序本身是对的，
 * 所以动的不是栈——给建议一行并列位置，有就出现、没有就整行不渲染，且不抢栈顶的主按钮。
 *
 * 返回 `null` = 此刻没有可摆的建议（没有建议，或它已经是栈顶那一态、不必说两遍）。
 */
export function pulseTopicSuggestion(
  pulse: KnowledgePulseDto,
  top: KnowledgePulseState,
): TopicSuggestionDto | null {
  if (top.kind === "topic") {
    return null;
  }
  return pulse.topics[0] ?? null;
}

/**
 * 两个来源 → **优先级栈，只渲染栈顶一态**（PRD-16-027 下一步引擎）。
 *
 * 顺序即「此刻最该做的一件事」：待整理 → 主题页建议 → 全部沉淀 → 空库。
 * **待整理排在最前**：还没整理的资料是流水线上游。往既有主题页补内容在那一页上就地
 * 预览确认（ADR-134），不经脉搏转达。
 *
 * `empty` 桶（无正文的资料）不参与判定——它既不可整理也不算沉淀。
 */
export function pulseState(pulse: KnowledgePulseDto): KnowledgePulseState {
  const { distill, topics } = pulse;
  if (distill.pending > 0) {
    return { kind: "pending", pending: distill.pending, edited: distill.edited };
  }
  const [first, ...rest] = topics;
  if (first) {
    return { kind: "topic", suggestion: first, more: rest.length };
  }
  if (distill.fresh > 0 || distill.edited > 0) {
    return { kind: "settled", fresh: distill.fresh, edited: distill.edited };
  }
  return { kind: "empty" };
}
