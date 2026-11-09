/**
 * 创建动作的**唯一前端注册表**（PRD-07-067/068/069 / TDD-07 §2.2）。
 *
 * 「添加内容」是完整目录，启动概览「快速开始」是从同一目录选出的子集——两处消费这一份
 * 表。此前概览自持一份私有 `QUICK_ACTIONS`，于是"有哪些添加方式"这个问题在应用里有两个
 * 答案，而它们从第一天起就不一样（那份表里有实时字幕和翻译，它们根本不创建任何东西）。
 *
 * **这里只有图标、文案与 handler**：id 清单来自 Rust（`CreateActionIdDto`，由
 * `luna-config` 的纯数据词表生成），可用性、原因与修复动作来自 `create_action_states`。
 * 前端不拥有其中任何一项——`Record<CreateActionIdDto, …>` 的穷尽性保证 Rust 加一个动作时
 * 这里当场编译不过，而不是悄悄少一格。
 */

import type { ReactNode } from "react";
import {
  commands,
  type CreateActionIdDto,
  type CreateActionReasonDto,
  type CreateActionRepairDto,
  type WorkspaceMetricEntryResultDto,
} from "../../ipc/bindings";
import { DiscIcon, DownloadIcon, MicIcon, NotebookIcon, PencilIcon } from "@lunarca/editor-core/Icon";
import { goTo } from "../../lib/navigate";

/**
 * 触发一个动作时要用到的应用级能力（由宿主注入，注册表自己不持有状态）。
 *
 * **每一条都要如实回报自己的终态**（PRD-07-070）。第一版全都返回 `void`，于是两个
 * 呈现面只能一律记成 `arrived`——那让「入口到达率」这条指标**只可能是 100%**，一个恒为
 * 1 的比率比没有这条指标更糟：它看起来在回答问题，实际上什么都没测。
 * 取消（文件对话框关掉）、被拒（已有语音会话）、失败（命令报错）都是真实且常见的结局。
 */
export interface CreateActionContext {
  /** 文字速记：落到「速记」并把光标放进那里的捕获胶囊。 */
  captureText: () => Promise<WorkspaceMetricEntryResultDto>;
  /** 语音速记：唤起 HUD，本次预设为存成速记（PRD-02-020）。 */
  captureVoice: () => Promise<WorkspaceMetricEntryResultDto>;
  /** 新建笔记：落到「笔记」。 */
  createNote: () => Promise<WorkspaceMetricEntryResultDto>;
  /** 开始录音：唤起录音 HUD，产物落「录音」。 */
  startRecording: () => Promise<WorkspaceMetricEntryResultDto>;
  /** 导入资料：音视频落「转写」、文档落「笔记」里的文档文件夹（ADR-164 ⑩）。 */
  importMaterial: () => Promise<WorkspaceMetricEntryResultDto>;
}

export interface CreateActionSpec {
  /** 图标——**一动作一个，绝不复用**：两个麦克风图标等于没有图标（PRD-07-069）。 */
  icon: ReactNode;
  /** 动作名。 */
  labelKey: string;
  /**
   * 产物去向（PRD-07-069）。
   *
   * **它不是装饰**：「语音速记」与「开始录音」都是对着麦克风说话，差别全在产物——
   * 一条短想法 vs 一段持续录音。不写出去向，用户只能靠两个麦克风图标猜。
   */
  destinationKey: string;
  /** 执行：只路由或唤起，**不在这里做业务**（ADR-003）；如实回报终态。 */
  run: (ctx: CreateActionContext) => Promise<WorkspaceMetricEntryResultDto>;
}

/**
 * 五类动作的呈现与执行。
 *
 * **没有 `order` 字段**：呈现顺序来自 Rust——「添加内容」按 `create_action_states`
 * 返回的数组顺序，「快速开始」按用户配置的数组顺序。在这里再写一份顺序，就会出现
 * 「设置里排第一、Popover 里排第三」这种两处各说各话。
 */
export const CREATE_ACTIONS: Record<CreateActionIdDto, CreateActionSpec> = {
  text_quick_note: {
    icon: <PencilIcon size={18} />,
    labelKey: "create.textQuickNote",
    destinationKey: "create.destination.quickNotes",
    // **不在 Popover 里嵌第二个编辑器**（PRD-07-067 ②）：跳到「速记」并聚焦那里本来
    // 就有的捕获框。嵌一个的代价是两套草稿状态、两套快捷键、两处"存哪儿去了"。
    run: (ctx) => ctx.captureText(),
  },
  voice_quick_note: {
    icon: <MicIcon size={18} />,
    labelKey: "create.voiceQuickNote",
    destinationKey: "create.destination.quickNotes",
    run: (ctx) => ctx.captureVoice(),
  },
  new_note: {
    icon: <NotebookIcon size={18} />,
    labelKey: "create.newNote",
    destinationKey: "create.destination.allNotes",
    run: (ctx) => ctx.createNote(),
  },
  start_recording: {
    icon: <DiscIcon size={18} />,
    labelKey: "create.startRecording",
    destinationKey: "create.destination.recordings",
    run: (ctx) => ctx.startRecording(),
  },
  import_material: {
    icon: <DownloadIcon size={18} />,
    labelKey: "create.importMaterial",
    destinationKey: "create.destination.imports",
    run: (ctx) => ctx.importMaterial(),
  },
};

/**
 * 暂时不可用的原因 → 文案 key（**穷尽**）。
 *
 * Rust 下发的是封闭枚举而不是一句话，正是为了让这句话能被本地化、也能随呈现面调整。
 * 少一格当场编译不过——而"缺一格"的运行时表现是一个不给任何理由的灰按钮。
 */
export const CREATE_ACTION_REASON_KEYS: Record<CreateActionReasonDto, string> = {
  vault_unavailable: "create.reason.vaultUnavailable",
  microphone_permission_required: "create.reason.micPermissionRequired",
  microphone_permission_denied: "create.reason.micPermissionDenied",
  audio_busy: "create.reason.audioBusy",
  voice_session_busy: "create.reason.voiceSessionBusy",
  recording_in_progress: "create.reason.recordingInProgress",
  asr_unavailable: "create.reason.asrUnavailable",
  service_consent_required: "create.reason.serviceConsentRequired",
  unsupported_platform: "create.reason.unsupportedPlatform",
};

/**
 * 执行一个修复动作（TDD-07 §2.2）。
 *
 * **两个呈现面共用这一份**：「添加内容」与「快速开始」都要给"去解决"，而修复的语义
 * （去哪个设置、申请哪个权限、回到哪个会话）与呈现面无关。各写一遍的表现是两处对同一个
 * 原因给出不同的下一步。
 *
 * @returns 这次修复是否**把用户带走了**——带走了，宿主浮层/弹窗就该收起；只是弹了一个
 * 系统授权框（用户还在原地、马上要重试）就不该收，否则他得重新打开一次才能点。
 */
export function runCreateActionRepair(
  repair: CreateActionRepairDto,
  ctx: CreateActionContext,
): boolean {
  switch (repair.kind) {
    case "open_settings":
      // route 的 wire 值就是深链本体——原样交出去，不拼 URL（TDD-07 §2.2）。
      goTo(repair.route);
      return true;
    case "request_microphone_permission":
      void commands.requestPermission("microphone");
      return false;
    case "show_voice_session":
      ctx.captureVoice();
      return true;
    case "show_recording_session":
      // 「回到录音」（B2）：只把常驻面板显示出来，会话原样——与入口的「开始录音」走同一条
      // 命令，开始/停止仍由用户在 HUD 内决定。
      void ctx.startRecording();
      return true;
  }
}

/**
 * 修复钮的文案 key（**穷尽**，按修复动作分）。
 *
 * 大多数修复是「去解决」——去授权、去设置。「回到录音」不是解决什么，是回到用户自己
 * 刚离开的现场（B2）；把它也叫「去解决」，等于告诉他这场录音是个问题。
 */
export const CREATE_ACTION_REPAIR_LABEL_KEYS: Record<CreateActionRepairDto["kind"], string> = {
  open_settings: "create.repair",
  request_microphone_permission: "create.repair",
  show_voice_session: "create.repair",
  show_recording_session: "create.repairReturnToRecording",
};
