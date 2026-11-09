/**
 * 处理队列任务的展示标签与状态谓词。DRY：面板 / 顶栏角标 / 回放内联状态共用同一判定。
 */

import type { ProcTaskDto } from "../../ipc/bindings";
import { formatBytes } from "@lunarca/editor-core/format-bytes";

/**
 * 是否**未终态**（排队 / 运行中）——顶栏角标、spinner、互斥按钮态的唯一口径。
 *
 * 用「终态白名单取反」而非枚举活跃态：新增终态（如 `cancelled`）时不会有某处漏改而
 * 把已结束的任务永远算作进行中（该缺陷曾在引入 `cancelled` 时同时出现在两处）。
 */
export function isTaskActive(task: ProcTaskDto): boolean {
  const state = task.status.state;
  return state !== "done" && state !== "failed" && state !== "cancelled";
}

/** 一篇笔记此刻的处理状态（行内状态胶囊）：在跑、排着、失败了，或没有任务。 */
export type NoteTaskState = "running" | "queued" | "failed" | null;

/**
 * 按 `note_rel` 找这一篇的任务：有未终态的取它（在跑 / 排着），否则看有没有失败的。录音 / 转写
 * 清单与创作「素材与来源」共用这一个口径。
 */
export function noteTaskState(items: readonly ProcTaskDto[], path: string): NoteTaskState {
  const active = items.find((task) => task.note_rel === path && isTaskActive(task));
  if (active) {
    return active.status.state === "running" ? "running" : "queued";
  }
  return items.some((task) => task.note_rel === path && task.status.state === "failed") ? "failed" : null;
}

/** 「已完成」段：成功终态。与 `isTaskUnsuccessful` 互斥、与 `isTaskActive` 互斥，三者穷尽。 */
export function isTaskDone(task: ProcTaskDto): boolean {
  return task.status.state === "done";
}

/**
 * 「未成功」段：失败 / 取消——待重试的工作，不是历史（PRD-04-055 三段修订）。
 * 与 Rust `ProcStatus::is_unsuccessful` 同口径。
 */
export function isTaskUnsuccessful(task: ProcTaskDto): boolean {
  const state = task.status.state;
  return state === "failed" || state === "cancelled";
}

/**
 * 是否值得给「重试」——**判据来自 Rust**（`ProcStatus::Failed.retryable`，按错误码判定），
 * 前端只读不判（ADR-003）。
 *
 * 取消恒可重试（用户意图，环境未变）；失败看下发的布尔——站点已下架 / 区域限制 / 登录墙 /
 * 正在直播这类**站点侧硬事实**重试只会重复失败，给按钮等于诱导用户空等（PRD-05-006）。
 * 曾经这里对所有非 done 项一律给重试，而"可重试集合"在 Rust 与前端各写一份、两边都没接线。
 */
export function isTaskRetryable(task: ProcTaskDto): boolean {
  // 上次会话回填的历史没有载荷（PRD-04-062 刻意不做失败自动重试），Rust 的 retry 也拒绝。
  if (task.restored) return false;
  const status = task.status;
  if (status.state === "cancelled") return true;
  return status.state === "failed" && status.data.retryable;
}

type Translate = (key: string, options?: Record<string, unknown>) => string;

/** 任务类型标签（导入 / 二次转写 / 二次翻译 / 说话人细分）。 */
export function procKindLabel(kind: string, t: Translate): string {
  return t(`processing.kind.${kind}`);
}

/** 运行中的阶段标签（如「转写中」）；非运行态回退纯类型标签。任务面板用——类型已在行首。 */
export function procPhaseLabel(task: ProcTaskDto, t: Translate): string {
  if (task.status.state !== "running") return procKindLabel(task.kind, t);
  return t(`processing.phase.${task.status.data.step}`);
}

/** 运行中标签「类型 · 阶段」（如「二次转写 · 转写中」）；非运行态回退纯类型标签。 */
export function procRunningLabel(task: ProcTaskDto, t: Translate): string {
  if (task.status.state !== "running") return procKindLabel(task.kind, t);
  return t("processing.kindPhase", {
    kind: procKindLabel(task.kind, t),
    phase: t(`processing.phase.${task.status.data.step}`),
  });
}

/**
 * 运行中的进度读数（**按阶段分派单位**）。`null` = 不定量（无读数，只转圈）。
 *
 * `done`/`total` 的语义随阶段变：转写是分片数、转译是分句数、而**下载阶段是 KiB**
 * （`ProcStatus::Running` 的计数是 u32，字节数轻易越过 4GiB，故后端以 KiB 上报，
 * 见 `ProcPhase::Fetching`）。裸着显示会变成「12678 / 90234」这种没人看得懂的数字，
 * 故下载阶段格式化成「12.4 MiB / 88.1 MiB」。
 */
export function procProgressCount(task: ProcTaskDto, locale: string): string | null {
  if (task.status.state !== "running") return null;
  const { step, done, total } = task.status.data;
  if (step === "fetching") {
    // 总量未知（服务器不给 Content-Length 且无估算）→ 只报已下载量，不编造分母。
    const downloaded = formatBytes(done * 1024, locale);
    return total > 0 ? `${downloaded} / ${formatBytes(total * 1024, locale)}` : downloaded;
  }
  return total > 0 ? `${done} / ${total}` : null;
}

/**
 * 服务端排位的一行文案：「排队第 N 位 · 预计 X–Y 分钟」（ADR-049 ⑧）。
 *
 * `null` = 这件任务此刻不在服务端队列里（本机跑的，或服务端已经开始解码）。
 *
 * **没有等待区间时只报位次**，绝不补一个估出来的时间：服务端在还没跑过任何一件时
 * 给不出样本，那时编一个数会被用户当成承诺（同 Rust 侧「不编解码百分比」的那条）。
 * 判据由 Rust 下发——两个界要么一起有、要么一起没有，故这里只判一次。
 *
 * 秒 → 分钟的换算与措辞是**呈现**，不是业务：排位、档位与区间本身全部由 Rust 给出，
 * 这里只决定「怎么把它读出来」（同 `formatBytes`）。
 */
export function procServerQueueLabel(task: ProcTaskDto, t: Translate): string | null {
  const queue = task.queue;
  if (!queue) return null;
  const { position, eta_min_secs: min, eta_max_secs: max } = queue;
  if (min === null || max === null) {
    return t("processing.serverQueue.position", { position });
  }
  // 一分钟以内说「不到 1 分钟」而不是「0–1 分钟」：后者读起来像「可能马上、也可能永远」。
  if (max < 60) return t("processing.serverQueue.soon", { position });
  const lo = Math.max(1, Math.round(min / 60));
  const hi = Math.max(lo, Math.round(max / 60));
  return lo === hi
    ? t("processing.serverQueue.etaSingle", { position, minutes: lo })
    : t("processing.serverQueue.eta", { position, min: lo, max: hi });
}

/** 档位的解释性说明（鼠标悬停）：回答「为什么我排在后面」。 */
export function procServerQueueTierHint(task: ProcTaskDto, t: Translate): string | undefined {
  const tier = task.queue?.tier;
  return tier === undefined ? undefined : t(`processing.serverQueue.tier.${tier}`);
}
