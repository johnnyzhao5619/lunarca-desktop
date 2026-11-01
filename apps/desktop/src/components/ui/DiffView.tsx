/**
 * 统一行差异呈现（unified diff）——**唯一一份**。
 *
 * 两处消费同一形态：版本历史的「回滚会改掉现在的什么」（PRD-06-041）与蒸馏重生成的
 * 「重新整理会改掉我写的什么」（PRD-06-045）。两处各画一份必然漂移（一处改了行号列宽、
 * 另一处没改），故收敛为一个组件；差异计算全在 Rust（`luna_vault::diff_lines`），
 * 本组件只渲染（ADR-003）。
 *
 * 降级与"两版相同"由调用方按各自语境措辞——同一句"没有差异"在版本历史里意味着
 * "回滚什么都不会变"，在蒸馏预览里意味着"重新生成产出与现在一致"，不是一句话。
 */

import type { LineDiffDto } from "../../ipc/bindings";

export function DiffView({ diff }: { diff: LineDiffDto }) {
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
