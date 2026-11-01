import type {
  LandingMode,
  VoiceInputEvent,
  VoiceInputOutputKind,
  VoiceInputState,
  VoiceInputStatus,
} from "../../ipc/bindings";
import type { TaskStatusKind } from "../../components/TaskStatus";

export type VoicePrimaryAction =
  | { kind: "start"; labelKey: "action.start"; disabled: false }
  | { kind: "stop"; labelKey: "action.stop"; disabled: boolean }
  | { kind: "processing"; labelKey: "action.processing"; disabled: true };

export type VoiceStatusTone = "ready" | "live" | "processing" | "success" | "danger";

export type TranscriptFeedback = {
  raw: string;
  finalText: string;
  refined: boolean;
  outputKind: VoiceInputOutputKind;
};

export type VoicePreviewLabelKey = "preview.raw" | "preview.refined" | "preview.translated";

export type VoiceLandingAction = {
  mode: LandingMode;
  labelKey: "landing.inject" | "landing.clipboard" | "landing.quick_note";
  isDefault: boolean;
};

export type VoiceModePatch = {
  landingMode?: VoiceInputState["landing_mode"];
  inputMode?: VoiceInputState["input_mode"];
  autoRefine?: boolean;
  translateEnabled?: boolean;
  translateTargetLang?: string;
};

export type VoiceModePayload = {
  landingMode: VoiceInputState["landing_mode"];
  inputMode: VoiceInputState["input_mode"];
  autoRefine: boolean;
  translateEnabled: boolean;
  translateTargetLang: string;
};

export function applyVoiceModePatch(state: VoiceInputState, patch: VoiceModePatch): VoiceModePayload {
  return {
    landingMode: patch.landingMode ?? state.landing_mode,
    inputMode: patch.inputMode ?? state.input_mode,
    autoRefine: patch.autoRefine ?? state.auto_refine,
    translateEnabled: patch.translateEnabled ?? state.translate_enabled,
    translateTargetLang: patch.translateTargetLang ?? state.translate_target_lang,
  };
}

export function isVoiceSessionActive(status: VoiceInputStatus): boolean {
  return status === "recording" || status === "recognizing" || status === "landing";
}

export function buildVoicePrimaryAction(state: VoiceInputState): VoicePrimaryAction {
  // 识别/落地阶段主按钮不可再"完成"（PRD-02-016）：给明确处理态；取消/重说仍可用。
  if (state.status === "recognizing" || state.status === "landing") {
    return {
      kind: "processing",
      labelKey: "action.processing",
      disabled: true,
    };
  }
  if (isVoiceSessionActive(state.status)) {
    return {
      kind: "stop",
      labelKey: "action.stop",
      disabled: false,
    };
  }
  return {
    kind: "start",
    labelKey: "action.start",
    disabled: false,
  };
}

export function voiceStatusTone(status: VoiceInputStatus): VoiceStatusTone {
  switch (status) {
    case "recording":
      return "live";
    case "recognizing":
    case "landing":
      return "processing";
    case "copied_fallback":
    case "cancelled":
      return "success";
    case "error":
    case "mic_busy":
      return "danger";
    case "idle":
    default:
      return "ready";
  }
}

export function voiceTaskStatusKind(status: VoiceInputStatus): TaskStatusKind {
  switch (status) {
    case "recording":
    case "recognizing":
    case "landing":
      return "processing";
    case "mic_busy":
      return "needs_action";
    case "copied_fallback":
      return "completed";
    case "error":
      return "failed";
    case "cancelled":
      return "cancelled";
    case "idle":
    default:
      return "idle";
  }
}

/** 按住说话期间的实时识别反馈（PRD-02-004）：committed 累计 + 可改写暂定尾。 */
export type LivePartial = {
  text: string;
  hypothesis: string;
};

/**
 * 实时识别反馈状态机（纯逻辑）：`partial` 事件全量替换；取消清空；最终结果（transcript）
 * 到达后由结果区接管，实时区随之清空。
 *
 * **不在 `status === "recording"` 上清空**。Rust 在一场会话**进行中**会反复重发
 * Recording 态（每一条电平诊断、等额度、额度到手都各发一次），它们只是同一场会话的状态
 * 刷新，不是"新会话开始"。此前这里把每一次都当成新会话清空实时文字，真机表现是：几个字
 * 刚出现就消失、再出现、再消失，直到松开后一次性给出全文。新会话的清空由 `startVoice`
 * 在发起会话那一刻显式做（那里才知道"这是新一场"），状态机不再猜。
 */
export function nextLivePartial(
  current: LivePartial | null,
  event: VoiceInputEvent,
): LivePartial | null {
  if (event.kind === "partial") {
    return { text: event.data.text, hypothesis: event.data.hypothesis };
  }
  if (event.kind === "transcript") {
    return null;
  }
  if (event.kind === "state" && event.data.status === "cancelled") {
    return null;
  }
  return current;
}

/** 结果反馈状态机：同上，只在取消时清空；新会话的清空归 `startVoice`。 */
export function nextTranscriptFeedback(
  current: TranscriptFeedback | null,
  event: VoiceInputEvent,
): TranscriptFeedback | null {
  if (event.kind === "transcript") {
    return {
      raw: event.data.raw,
      finalText: event.data.final_text,
      refined: event.data.refined,
      outputKind: event.data.output_kind,
    };
  }
  if (event.kind === "state" && event.data.status === "cancelled") {
    return null;
  }
  return current;
}

/**
 * 新转写结果才替换编辑稿；落地失败仅更新会话状态，保持同一结果引用，用户的校正内容不能丢失。
 */
export function nextEditedVoiceOutput(
  currentEditedText: string,
  currentFeedback: TranscriptFeedback | null,
  nextFeedback: TranscriptFeedback | null,
): string {
  return currentFeedback === nextFeedback ? currentEditedText : (nextFeedback?.finalText ?? "");
}

/**
 * 电平条何时归零：取消、出错，以及**最终结果到达**（结果态冻结着最后几帧会像还在录，真机 C-7）。
 * 同样**不在 Recording 态刷新时归零**——那会让波形随每条
 * 电平诊断闪一下；新会话的归零由 `startVoice` 显式做。
 */
export function shouldResetVoiceLevels(event: VoiceInputEvent): boolean {
  if (event.kind === "transcript") return true;
  return (
    event.kind === "state" &&
    (event.data.status === "cancelled" || event.data.status === "error")
  );
}

export function voiceLandingActions(defaultMode: LandingMode): VoiceLandingAction[] {
  return [
    { mode: "inject", labelKey: "landing.inject", isDefault: defaultMode === "inject" },
    { mode: "clipboard", labelKey: "landing.clipboard", isDefault: defaultMode === "clipboard" },
    {
      mode: "quick_note",
      labelKey: "landing.quick_note",
      isDefault: defaultMode === "quick_note",
    },
  ];
}

export function voiceFeedbackLabelKey(feedback: TranscriptFeedback | null): VoicePreviewLabelKey {
  if (!feedback || feedback.outputKind === "raw") {
    return "preview.raw";
  }
  if (feedback.outputKind === "translated") {
    return "preview.translated";
  }
  return "preview.refined";
}

/** 东亚宽字符（CJK 统一表意 / 假名 / 谚文 / 全角标点）按两个单位计宽，其余一个。 */
function textUnits(line: string): number {
  let units = 0;
  for (const ch of line) {
    units += /[\u1100-\u115f\u2e80-\ua4cf\uac00-\ud7a3\uf900-\ufaff\ufe30-\ufe4f\uff00-\uff60\uffe0-\uffe6]/u.test(ch)
      ? 2
      : 1;
  }
  return units;
}

/**
 * 结果编辑框的行数（用 `rows` 而不是量 scrollHeight 回写 style：后者是内联样式，且每次输入都
 * 要读一次布局）。按**宽度单位**折行而不是字符数：CJK 一个字占两个 ASCII 的宽，此前按
 * 「每行 42 字符」估，中文结果的行数高估近一倍，短句下面留一片空白（真机 R-23）。
 * `unitsPerLine` 是一行能放的 ASCII 单位数（440 宽的 HUD、14px 字号约 56）。
 */
export function estimateTextRows(
  text: string,
  { unitsPerLine, min, max }: { unitsPerLine: number; min: number; max: number },
): number {
  const rows = text
    .split("\n")
    .reduce((sum, line) => sum + Math.max(1, Math.ceil(textUnits(line) / unitsPerLine)), 0);
  return Math.min(max, Math.max(min, rows));
}
