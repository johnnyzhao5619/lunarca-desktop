/**
 * 统一行差异呈现（unified diff）——**唯一一份**，桌面与网页共用（TDD-17 §2）。
 *
 * 桌面三处消费同一形态：版本历史的「回滚会改掉现在的什么」（PRD-06-041）、蒸馏重生成的「重新整理会改掉我写的
 * 什么」（PRD-06-045）与外部改动 / 冲突副本的对比；网页「别处改了这篇」的「查看对比」（PRD-21-030）。差异
 * 计算全在 Rust（`luna_sync_core::diff::diff_lines`；桌面经 IPC、网页经 Worker 的 WASM），本组件只渲染
 * （ADR-003）。
 *
 * 降级与"两版相同"由调用方按各自语境措辞——同一句"没有差异"在版本历史里意味着"回滚什么都不会变"，在蒸馏
 * 预览里意味着"重新生成产出与现在一致"，不是一句话。
 */

/** 一行差异（与 Rust `DiffLine` 的线上形状同形）：`kind` 为 `equal` / `added` / `removed`。 */
export interface LineDiffLine {
  kind: string;
  old_line: number | null;
  new_line: number | null;
  text: string;
}

/** 一次对比的结果（与 Rust `LineDiff` 同形）；`degraded` = 差异区过大，已降级为整段替换。 */
export interface LineDiff {
  lines: LineDiffLine[];
  added: number;
  removed: number;
  degraded: boolean;
}

export function DiffView({ diff }: { diff: LineDiff }) {
  return (
    <div className="luna-diff">
      {diff.lines.map((line, index) => (
        <div
          key={`${line.kind}-${line.old_line ?? "_"}-${line.new_line ?? "_"}-${index}`}
          className="luna-diff__line"
          data-kind={line.kind}
        >
          <span className="luna-diff__no">{line.old_line ?? ""}</span>
          <span className="luna-diff__no">{line.new_line ?? ""}</span>
          <span className="luna-diff__sign" aria-hidden="true">
            {line.kind === "added" ? "+" : line.kind === "removed" ? "−" : " "}
          </span>
          <span className="luna-diff__text">{line.text}</span>
        </div>
      ))}
    </div>
  );
}
