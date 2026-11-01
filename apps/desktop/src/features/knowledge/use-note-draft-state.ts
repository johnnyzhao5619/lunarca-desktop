/**
 * 「这一篇此刻的成熟度动作可用性」的**唯一取数口径**（ADR-086 / PRD-06-053 / PRD-06-058）。
 *
 * 两个消费面：页头「记录」徽标（一键沉淀）与编辑器右键「标记为记录」。它们问的是同一篇
 * 笔记的同一件事，故取数、订阅与失效逻辑只写一份——各写一份的代价不是重复，而是**两处
 * 会答出不同的话**（右键说能标记，徽标却已经显示它是记录态）。
 *
 * 判定在 Rust（`note_draft_state` 下发两个已判定的布尔），前端不认识 `luna.stage` 的任何
 * 取值（ADR-003 / `scripts/check-note-stage-single-source.py`）。
 */

import { useCallback, useEffect, useState } from "react";
import { commands, events } from "../../ipc/bindings";

export interface NoteDraftState {
  /** 它是记录态吗（页头徽标 = 一键沉淀的判据）。 */
  draft: boolean;
  /** 它能被「标记为记录」吗（右键那一项的判据）。 */
  markableAsDraft: boolean;
  /** 手动重取（沉淀/标记之后调用方立刻要看到新状态）。 */
  refresh: () => Promise<void>;
}

const ABSENT = { draft: false, markableAsDraft: false };

export function useNoteDraftState(notePath: string | null): NoteDraftState {
  const [state, setState] = useState(ABSENT);

  const refresh = useCallback(async () => {
    if (!notePath) {
      setState(ABSENT);
      return;
    }
    const result = await commands.noteDraftState(notePath);
    setState(
      result.status === "ok"
        ? { draft: result.data.draft, markableAsDraft: result.data.markable_as_draft }
        : ABSENT,
    );
  }, [notePath]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // 用户在正文里手改 frontmatter（加上/删掉 `luna.stage`）后两个消费面都要跟着变。
  // **按路径过滤**（同 `DistillBadge` 的先例）：两条都是全库写盘信号，不过滤的话批量
  // 动作期间会白发上百次判定。
  //
  // **必须两条事件都听**：编辑器内保存（`write_note`）只发 `note://written`——写盘的
  // 自写回声在 `vault://changed` 一侧被抑制；外部编辑器改文件与程序化改写（沉淀 / 标记为
  // 记录 / 整理）则走 `vault://changed`。只听后者，用户在正文里敲下 `stage` 保存后徽标
  // 不会出现（2026-08-24 批次 5 验收发现）。
  useEffect(() => {
    const unlistenVault = events.vaultChanged.listen((event) => {
      if (event.payload.rel_path === notePath) {
        void refresh();
      }
    });
    const unlistenWritten = events.noteWritten.listen((event) => {
      if (event.payload.path === notePath) {
        void refresh();
      }
    });
    return () => {
      void unlistenVault.then((off) => off());
      void unlistenWritten.then((off) => off());
    };
  }, [notePath, refresh]);

  return { ...state, refresh };
}
