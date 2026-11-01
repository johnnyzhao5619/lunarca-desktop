/**
 * 统一处理进度条（PRD-04-055）：旋转指示 + 标签 + 已完成/总数 + 原生 `<progress>` 进度条。
 * 确定性 = 有计数（分片/分句）；不定量 = total=0（导入/二次转写的转译阶段）。
 * 任务面板与回放视图内联状态共用，呈现完全对齐。
 *
 * 标签有两档：`kindPhase`（「类型 · 阶段」，回放视图内联——那里没有别处写类型）与
 * `phase`（只写阶段，任务面板——类型已在行首胶囊里，再写一遍是噪声）。
 */

import { useTranslation } from "react-i18next";
import type { ProcTaskDto } from "../../ipc/bindings";
import { procPhaseLabel, procProgressCount, procRunningLabel } from "./processing-labels";

export function ProcessingProgress({
  task,
  label = "kindPhase",
}: {
  task: ProcTaskDto;
  label?: "kindPhase" | "phase";
}) {
  const { t, i18n } = useTranslation("workspace");
  if (task.status.state !== "running") return null;
  const { done, total } = task.status.data;
  const determinate = total > 0;
  // 读数单位随阶段变（下载是字节、转写是分片），故交给纯函数分派，不在此写死 `done / total`。
  const count = procProgressCount(task, i18n.language);
  return (
    <div className="luna-proc-progress" role="status" aria-live="polite">
      <div className="luna-proc-progress__head">
        <span className="luna-proc-progress__spinner" aria-hidden="true" />
        <span className="luna-proc-progress__label">
          {label === "phase" ? procPhaseLabel(task, t) : procRunningLabel(task, t)}
        </span>
        {count !== null && <span className="luna-proc-progress__count">{count}</span>}
      </div>
      <progress
        className="luna-progress"
        aria-label={t("processing.progressAria")}
        max={determinate ? total : undefined}
        value={determinate ? done : undefined}
      />
    </div>
  );
}
