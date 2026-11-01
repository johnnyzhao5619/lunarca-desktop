/**
 * 一次状态面到访的观测（PRD-07-070 / TDD-12 §2.1）。
 *
 * **住在 `hooks/` 而不是某个 feature 里**：三条状态路径的宿主分属 `notifications` /
 * `processing` / `usage` 三个功能模块，把它放进其中任何一个，另外两个就要跨功能去 import
 * ——`usage → workspace` 这条边当场让功能模块图成环（`settings → usage → workspace →
 * editor → popup-translate → settings`），而结构门禁会红。它本来也不属于任何一个功能：
 * 它是"记一条观测"这件横切的事。
 *
 * **前端不判断能不能记**：授权判定在 Rust（关着时命令静默 no-op）。让前端先问一次
 * "能不能记"再决定要不要调，等于每个观测点都多一次往返。
 */

import { useCallback, useEffect, useRef, type KeyboardEvent, type PointerEvent } from "react";
import { commands, type WorkspaceMetricStatusSurfaceDto } from "../ipc/bindings";

/** 宿主拿到的两样东西：一个显式标记函数，一组摊在内容容器上的捕获监听。 */
export interface StatusVisitMetrics {
  /** 宿主里那些"算作操作"的动作可以显式调它（如面板自己的列表项回调）。 */
  markInteracted: () => void;
  /** 摊到面板**内容**容器上（不是遮罩、不是关闭钮）。 */
  interactionProps: {
    onPointerDownCapture: (event: PointerEvent) => void;
    onKeyDownCapture: (event: KeyboardEvent) => void;
  };
}

/**
 * 三条状态路径各自的**唯一宿主**调用它（§2.1「不在两个展示面或重复 effect 各报一次」）。
 *
 * **`interacted` 的判据写死在这里，不留给每个宿主自己发挥**：三个面板各写一套"算不算
 * 操作过"，这条指标就没法横向比较——而它存在的全部理由正是"哪一条状态路径打开了却用不
 * 起来"。判据是：**在面板内容里按下了指针，或在其上按了 Enter / 空格**（激活键）。
 *
 * 明确**不算**操作的：Esc、Tab、方向键、滚动、以及点在遮罩或关闭钮上——它们要么是"我
 * 走了"，要么是"我在找"，都不是把这个面板用起来了。
 *
 * **计时从打开算到关闭，且只属于这一次**（§2.1「计时器仅属于当前操作，不用于关联独立
 * 事件」）：原始毫秒不出这一次调用，Rust 只保留它落进哪个桶。**关闭时才上报**，故进程
 * 退出、组件被整棵卸载时那条在途观测直接丢掉、不补报——这正是 §2.1 的观测边界要的语义。
 */
export function useStatusVisitMetrics(
  surface: WorkspaceMetricStatusSurfaceDto,
  open: boolean,
): StatusVisitMetrics {
  const startedAt = useRef<number | null>(null);
  const startedSurface = useRef(surface);
  const interacted = useRef(false);
  const markInteracted = useCallback(() => {
    interacted.current = true;
  }, []);

  useEffect(() => {
    if (open) {
      // StrictMode 会重放 effect；已有开始点时保持同一次访问，不重置计时和 interacted。
      if (startedAt.current === null) {
        startedAt.current = performance.now();
        startedSurface.current = surface;
        interacted.current = false;
      }
      return;
    }
    // 只把显式 open → closed 当作终结。effect cleanup 同时覆盖 StrictMode 重放和整棵卸载，
    // 用它上报会制造一次零时长假访问，并违背“进程退出/卸载丢弃在途观测”的契约。
    const began = startedAt.current;
    startedAt.current = null;
    if (began === null) {
      return;
    }
    void commands.recordWorkspaceMetric({
      kind: "status_visit_finished",
      surface: startedSurface.current,
      interacted: interacted.current,
      duration_ms: Math.max(0, Math.round(performance.now() - began)),
    });
  }, [surface, open]);

  return {
    markInteracted,
    interactionProps: {
      onPointerDownCapture: markInteracted,
      onKeyDownCapture: (event: KeyboardEvent) => {
        if (event.key === "Enter" || event.key === " ") {
          markInteracted();
        }
      },
    },
  };
}
