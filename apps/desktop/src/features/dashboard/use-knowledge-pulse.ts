/**
 * 知识库脉搏的取数（判定在纯函数 `knowledge-pulse.ts`，呈现在 `KnowledgePulse.tsx`）。
 *
 * 一次取数（PRD-16-027）：脉搏渲染的是一个优先级栈的栈顶，分几次 IPC 取会让它先渲染
 * 一态、再跳成另一态——那正是「只渲染栈顶一行」想避免的抖动。
 *
 * 取数被提到 hook 里，是因为**谁拿主按钮**要在概览这一层裁决（`next-actions.ts`）：
 * 那条仲裁需要知道脉搏此刻是不是可行动态，而这件事此前只有脉搏组件自己知道。
 */

import { useCallback, useEffect, useState } from "react";
import { commands, type TopicSuggestionDto } from "../../ipc/bindings";
import { pulseState, pulseTopicSuggestion, type KnowledgePulseState } from "./knowledge-pulse";

export interface KnowledgePulseHandle {
  /** 栈顶那一态；`null` = 没有可说的（还没取到，或取数失败）——这一行不渲染。 */
  state: KnowledgePulseState | null;
  /**
   * 取数已有结论（成功或失败都算）。
   *
   * 与 `state !== null` **不是**一回事，而这个区别是有代价的：宿主要等三条来源都有
   * 结论才画整区，若把"取失败"永远留在"还没到"上，脉搏一失败就会把装机与体检那两行
   * 一起扣住——而装机那一行正是概览被弹出来的理由。
   */
  loaded: boolean;
  /** 栈顶之外并列的主题页建议。 */
  aside: TopicSuggestionDto | null;
  /** 忽略某条建议后重取（栈顶顺位补上，PRD-16-004）。 */
  reload: () => void;
}

/** 可行动的两态——settled / empty 只是陈述，不该占用主按钮。 */
export function isActionable(state: KnowledgePulseState | null): boolean {
  return state?.kind === "pending" || state?.kind === "topic";
}

export function useKnowledgePulse(): KnowledgePulseHandle {
  const [state, setState] = useState<KnowledgePulseState | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [aside, setAside] = useState<TopicSuggestionDto | null>(null);
  const [epoch, setEpoch] = useState(0);

  useEffect(() => {
    let disposed = false;
    void commands.knowledgePulse().then((pulse) => {
      if (disposed) {
        return;
      }
      if (pulse.status !== "ok") {
        // 取不到就是没有可说的：脉搏是增值信息，出不来不该占一行骨架屏，更不该扣住整区。
        setState(null);
        setAside(null);
        setLoaded(true);
        return;
      }
      const top = pulseState(pulse.data);
      setState(top);
      setAside(pulseTopicSuggestion(pulse.data, top));
      setLoaded(true);
    });
    return () => {
      disposed = true;
    };
  }, [epoch]);

  const reload = useCallback(() => setEpoch((n) => n + 1), []);
  return { state, loaded, aside, reload };
}
