import { useEffect, useMemo, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { IconButton } from "../../components/ui/IconButton";
import {
  commands,
  type PresetInfo,
  type TranslateEvent,
  type VoiceInputEvent,
} from "../../ipc/bindings";
import { useTranslation } from "react-i18next";
import { failureMessage, hasFailureMessage } from "../../i18n/failure";
import { keyToMessage } from "../voice-input/voice-message";
import { FailureCard } from "../../components/ui/FailureCard";
import { HudSurface } from "../../components/ui/HudSurface";
import { HudReceipt } from "../../components/ui/HudReceipt";
import { HudHeader } from "../../components/ui/HudHeader";
import { usePanelTextEditing } from "../../hooks/use-panel-text-editing";
import { useTablistNavigation } from "../../hooks/use-tablist-navigation";
import { PopupResultSlot } from "./PopupResultSlot";
import { PresetSelect } from "./PresetSelect";
import { presetLabel } from "./popup-meta-label";
import { type CompositionState, NO_COMPOSITION } from "../../lib/ime-composition";
import { classifyPopupInputKey } from "./popup-keys";
import { vaultErrorText } from "../../lib/vault-error";
import { idlePopupVoice, isPopupVoiceActive, popupVoiceOnReopen } from "./popup-voice";
import { SpeakButton } from "../tts/SpeakButton";
import {
  CameraIcon,
  ChevronDownIcon,
  CloseIcon,
  LanguagesIcon,
  MicIcon,
  PencilIcon,
  PinFilledIcon,
  PinIcon,
  PlusIcon,
  StopCircleIcon,
  StopIcon,
  SparklesIcon,
} from "../../components/ui/Icon";
import {
  applySlotCacheHit,
  applySlotDone,
  applySlotFailure,
  applySlotMeta,
  applySlotTerminal,
  applySlotToken,
  canAddCompareSlot,
  createInitialSlots,
  createPopupSlot,
  nextComparePresetId,
  removeCompareSlot as removeCompareSlotFrom,
  resultsVisible as slotsHaveResults,
  setSlotRunning,
  type PopupSlot,
} from "./popup-slots";

declare global {
  interface Window {
    __lunaRefreshPopup?: () => void;
  }
}

type PopupMode = "display" | "input";

type PopupTranslateViewProps = {
  presets: PresetInfo[];
};

/** 草稿写回 Rust 会话的防抖（PRD-01-005 修订）：失焦与关窗前另有冲刷，这里只管打字期间。 */
const DRAFT_SYNC_MS = 300;

function updateSlot(slots: PopupSlot[], slotId: number, update: (slot: PopupSlot) => PopupSlot) {
  return slots.map((slot) => (slot.id === slotId ? update(slot) : slot));
}

export function PopupTranslateView({ presets }: PopupTranslateViewProps) {
  const { t } = useTranslation("popup");
  // 语音输入的状态键由 Rust 现拼（`voice:status.*` / `errors:*`），文案住在 `voice-input`。
  const { t: tVoice } = useTranslation("voice-input");
  const presetIds = useMemo(() => presets.map((preset) => preset.id), [presets]);
  const [popupIn, setPopupIn] = useState("");
  const [popupMode, setPopupMode] = useState<PopupMode>("input");
  // 展示模式的会话源文本（原文可见 + 「编辑原文」预填，PRD-01-006）。
  const [popupSource, setPopupSource] = useState<string | null>(null);
  const [canReplaceSource, setCanReplaceSource] = useState(false);
  const [saveToTranslateSpace, setSaveToTranslateSpace] = useState(false);
  const [popupVoice, setPopupVoice] = useState(idlePopupVoice);
  const [slots, setSlots] = useState<PopupSlot[]>(() => createInitialSlots(presetIds));
  /** 对比时此刻看的是哪一槽（分段切换，ADR-137 H4）：两张结果卡不再在窄窗里纵向叠放。 */
  const [activeSlotId, setActiveSlotId] = useState<number | null>(null);
  /** 本次唤起是否固定（失焦不收，Rust `popup_set_pinned`；下一次唤起重新武装）。 */
  const [pinned, setPinned] = useState(false);
  /** 展示模式的原文默认折成一行（译文是主体，原文是核对时才展开的东西）。 */
  const [sourceExpanded, setSourceExpanded] = useState(false);
  /** 这一次取选区借用了剪贴板却没能原样还原（Rust 会话判定，ADR-163 ②）：如实告诉用户。 */
  const [clipboardNotRestored, setClipboardNotRestored] = useState(false);
  const popupInputRef = useRef<HTMLTextAreaElement | null>(null);
  /**
   * 合成边界（`popup-keys.ts` → `lib/ime-composition.ts` 的判据之一）。
   *
   * **存 ref 不存 state**：它只在按键那一刻被读一次，本身不该触发任何一次渲染——而中文
   * 输入每敲一个字母都会走一遍组字事件，做成 state 就是每个字母重渲一次。
   */
  const compositionRef = useRef<CompositionState>({ ...NO_COMPOSITION });
  const inputEditing = usePanelTextEditing();
  /**
   * 每个槽位当前这一次生成的代号。同槽位再生成（换预设 / 重新生成 / 重开弹窗）时 Rust 会
   * 取消上一次，但旧 channel 仍会把它的终态（`cancelled`）乃至在途 token 送过来——
   * 槽位 id 是复用的（恒为 1 / 2），不按代号过滤的话旧流会写进新结果。
   */
  const slotRunsRef = useRef<Map<number, number>>(new Map());
  // 复制成功反馈（2s 后自动重置 slot id → null；手动复制与自动复制共用）
  const [copiedSlotId, setCopiedSlotId] = useState<number | null>(null);
  const copyFeedbackTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  /**
   * 未生成的草稿归 Rust 会话（PRD-01-005 修订 2026-09-07）：此前输入只在「生成」时才写进
   * 会话，而关窗（Esc / 失焦）会清会话——打了半句按 Esc，重开是空的；注释却写着「重开即
   * 恢复」，那只对生成过的情形成立。打字期间防抖写回，失焦与关窗前冲刷。
   */
  const popupInRef = useRef("");
  const draftSyncedRef = useRef("");
  const draftTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  function markCopied(slotId: number) {
    if (copyFeedbackTimerRef.current) clearTimeout(copyFeedbackTimerRef.current);
    setCopiedSlotId(slotId);
    copyFeedbackTimerRef.current = setTimeout(() => {
      setCopiedSlotId(null);
      copyFeedbackTimerRef.current = null;
    }, 2000);
  }

  function flushDraft(value: string = popupInRef.current) {
    if (draftTimerRef.current) {
      clearTimeout(draftTimerRef.current);
      draftTimerRef.current = null;
    }
    if (draftSyncedRef.current === value) return;
    draftSyncedRef.current = value;
    void commands.popupSetDraft(value);
  }

  function scheduleDraftSync(value: string) {
    if (draftTimerRef.current) clearTimeout(draftTimerRef.current);
    draftTimerRef.current = setTimeout(() => {
      draftTimerRef.current = null;
      flushDraft(value);
    }, DRAFT_SYNC_MS);
  }

  function closePopup() {
    flushDraft();
    void commands.popupClose();
  }

  useEffect(() => {
    if (presetIds.length > 0 && slots.length === 0) {
      setSlots(createInitialSlots(presetIds));
    }
  }, [presetIds, slots.length]);

  useEffect(() => {
    if (popupMode === "input") {
      popupInputRef.current?.focus();
    }
  }, [popupMode]);

  useEffect(() => {
    const refresh = () => {
      void commands.popupSessionState().then((result) => {
        if (result.status === "ok") {
          applyPopupSession(
            result.data.mode,
            result.data.has_source,
            result.data.source_text,
            result.data.save_to_translate_space,
            result.data.draft,
            result.data.clipboard_not_restored,
          );
        }
      });
    };
    window.__lunaRefreshPopup = refresh;
    refresh();
    return () => {
      delete window.__lunaRefreshPopup;
    };
  }, [presets]);

  function applyPopupSession(
    mode: string,
    hasSource: boolean,
    sourceText: string | null,
    saveToTranslateSpace: boolean,
    draft: string | null,
    clipboardNotRestored: boolean,
  ) {
    const nextMode: PopupMode = mode === "display" ? "display" : "input";
    const nextSlots = createInitialSlots(presetIds);
    setPopupMode(nextMode);
    setPopupSource(sourceText);
    setClipboardNotRestored(clipboardNotRestored);
    setCanReplaceSource(hasSource);
    setSaveToTranslateSpace(saveToTranslateSpace);
    setSlots(nextSlots);
    setActiveSlotId(null);
    setPinned(false);
    setSourceExpanded(false);
    // 面板是预热复用的（ADR-015），语音状态跨「隐藏 → 再显示」会一直留着：不收回的话，
    // 上一次的「凭据被拒绝」会挂在输入框下方的提示位上，而用户这一次做的可能是纯文字翻译。
    setPopupVoice(popupVoiceOnReopen);
    if (nextMode === "display" && hasSource && nextSlots[0]) {
      void generateSlot(nextSlots[0].id, nextSlots[0].presetId, false, {
        useExistingSessionSource: true,
      });
    } else if (nextMode === "input") {
      // 输入模式：会话已有源文本（隐藏后重开/语音填入）则预填回输入框；否则预填未生成的
      // 草稿；都没有才清空。草稿视为已同步（它本来就来自 Rust）。
      const text = sourceText ?? draft ?? "";
      setInput(text);
      draftSyncedRef.current = draft ?? "";
      // 预填的光标置尾：用户是回来接着写的，不是回来重选的。
      requestAnimationFrame(() => {
        const el = popupInputRef.current;
        if (el) el.setSelectionRange(el.value.length, el.value.length);
      });
    }
  }

  function setInput(text: string) {
    popupInRef.current = text;
    setPopupIn(text);
  }

  // 「编辑原文」（PRD-01-006 选区/截屏源文可编辑后重译）：切输入模式并预填会话源文本。
  // 展示模式的窗口不收键盘（Windows 上不可激活）：先上报「开始编辑」，让 Rust 把它变成可打字的。
  function editSource() {
    inputEditing.onFocus();
    setPopupMode("input");
    setInput(popupSource ?? "");
    setSlots(createInitialSlots(presetIds));
  }

  async function ensureInputSource() {
    if (popupMode !== "input") {
      return true;
    }
    if (draftTimerRef.current) {
      clearTimeout(draftTimerRef.current);
      draftTimerRef.current = null;
    }
    const result = await commands.popupSetInput(popupIn);
    if (result.status === "error") {
      setSlots((current) => current.map((slot) => applySlotTerminal(slot, "error", result.error)));
      return false;
    }
    // 草稿已升格为源文本（Rust 侧随之清草稿）；本地同步位归零，后续再改才重新写回。
    draftSyncedRef.current = "";
    return true;
  }

  async function generateSlot(
    slotId: number,
    presetId: string,
    force: boolean,
    options: { useExistingSessionSource?: boolean } = {},
  ) {
    const hasSource = options.useExistingSessionSource ? true : await ensureInputSource();
    if (!hasSource) {
      return;
    }
    setSlots((current) => updateSlot(current, slotId, (slot) => setSlotRunning(slot, presetId)));
    const runId = (slotRunsRef.current.get(slotId) ?? 0) + 1;
    slotRunsRef.current.set(slotId, runId);
    const channel = new Channel<TranslateEvent>();
    channel.onmessage = (event) => {
      // 被顶替的旧流：Rust 已取消它，它的终态与残余 token 都不属于这一槽现在的结果。
      if (slotRunsRef.current.get(slotId) !== runId) {
        return;
      }
      // 自动复制回执（PRD-01-032）：只做反馈，不改槽位内容（副作用不进 setSlots updater）。
      if (event.kind === "auto_copied") {
        markCopied(slotId);
        return;
      }
      setSlots((current) =>
        updateSlot(current, slotId, (slot) => {
          switch (event.kind) {
            case "meta":
              return applySlotMeta(slot, event.data);
            case "token":
              return applySlotToken(slot, event.data);
            case "done":
              return applySlotDone(slot, event.data);
            case "cache_hit":
              return applySlotCacheHit(slot, event.data);
            case "failed":
              // 稳定错误码 → 本地化修复文案（绝不裸显 UPSTREAM 之类的码）；
              // 码本身随槽位保留，驱动错误卡上的修复动作按钮。
              return applySlotFailure(slot, event.data.code, failureMessage(event.data.code));
            case "cancelled":
              return applySlotTerminal(slot, "cancelled", null);
            case "started":
              return slot;
            // truncated / floating_menu / floating_chunk 是笔记 AI 悬浮菜单的事实
            // （PRD-06-020/025），弹窗翻译不发。
            case "truncated":
            case "floating_menu":
            case "floating_chunk":
            case "floating_rejected":
              return slot;
          }
        }),
      );
    };
    const result = await commands.popupGenerate(presetId, force, slotId, channel);
    if (result.status === "error" && slotRunsRef.current.get(slotId) === runId) {
      setSlots((current) =>
        updateSlot(current, slotId, (slot) => applySlotTerminal(slot, "error", result.error)),
      );
    }
  }

  /** 停止某一槽的生成；Rust 取消在途 Job，终态 `cancelled` 经同一 channel 回来。 */
  function stopSlot(slotId: number) {
    void commands.popupStop(slotId);
  }

  function generatePrimarySlot() {
    const primary = slots[0];
    if (!primary || popupIn.trim() === "") {
      return;
    }
    // 改原文时旧结果留在原处，提交这一下才换（ADR-137 H4）：此前每敲一个字结果区就清空、
    // 窗口缩回输入卡，用户在改一个词的时候看不到要对照的那句译文。对比槽随新原文作废。
    for (const slot of slots.slice(1)) {
      stopSlot(slot.id);
      slotRunsRef.current.delete(slot.id);
    }
    setSlots((current) => current.slice(0, 1));
    setActiveSlotId(null);
    void generateSlot(primary.id, primary.presetId, false);
  }

  async function togglePinned() {
    const result = await commands.popupSetPinned(!pinned);
    if (result.status === "ok") setPinned(result.data);
  }

  const voiceMessage =
    popupVoice.customMessage ?? (popupVoice.messageKey ? t(popupVoice.messageKey) : "");
  // 提示位只在语音**进行中**让位给语音状态（用户刚按下麦克风，那是此刻唯一相关的话）；
  // 空闲恒显键盘引导，失败走下面的错误卡——不再让上一次的失败原因把引导挤掉。
  const inputAssistText = isPopupVoiceActive(popupVoice) ? voiceMessage : t("input.hint");
  const voiceFailed = popupVoice.status === "error";

  function handlePopupVoiceEvent(event: VoiceInputEvent) {
    switch (event.kind) {
      case "state":
        // 主消息一律走 `message_key`（Rust 每处都填了）——此前是「有 `guidance_zh`
        // 就把它当消息直出」，而那个字段装的是中文句子，英文界面照样显示中文。
        // `message_detail` 只是补充细节（路径 / 原始错误串），语言无关。
        // `message_key` 是 Rust 现拼的内部键（`voice:status.*` / `errors:*`），**不是**本窗
        // 命名空间的键：此前直接交给 `t()`，i18next 把 `voice` 当成不存在的命名空间，原样吐出
        // 「status.recognizing」「invalid_input」这种裸键（2026-09-05 真机）。与语音 HUD 走同一份
        // 映射（`voice-message.ts`），认不出的码回中性兜底。
        setPopupVoice({
          status: event.data.status,
          messageKey: "status.voice_ready",
          customMessage: event.data.message_key
            ? keyToMessage(event.data.message_key, event.data.status, tVoice)
            : undefined,
        });
        break;
      case "field_input":
        {
          const nextSlots = createInitialSlots(presetIds);
          setInput(event.data.text);
          setPopupSource(event.data.text);
          setSlots(nextSlots);
          setPopupVoice(idlePopupVoice());
          if (nextSlots[0]) {
            void generateSlot(nextSlots[0].id, nextSlots[0].presetId, false, {
              useExistingSessionSource: true,
            });
          }
        }
        break;
      case "failed": {
        // 语音失败 → 唯一失败注册表（ADR-111 ④）；认不出的码回本窗的中性兜底。
        const code = event.data.code.toUpperCase();
        setPopupVoice({
          status: "error",
          messageKey: "status.voice_error",
          customMessage: hasFailureMessage(code) ? failureMessage(code) : undefined,
          failureCode: hasFailureMessage(code) ? code : undefined,
        });
        break;
      }
      case "landed":
        break;
    }
  }

  async function togglePopupVoiceInput() {
    if (isPopupVoiceActive(popupVoice)) {
      const result = await commands.voiceInputStop(null);
      if (result.status === "error") {
        setPopupVoice({
          status: "error",
          customMessage: result.error,
        });
      }
      return;
    }
    const channel = new Channel<VoiceInputEvent>();
    channel.onmessage = handlePopupVoiceEvent;
    const result = await commands.popupVoiceInputStart(channel);
    if (result.status === "ok") {
      setPopupVoice({
        status: result.data.status,
        messageKey: "status.voice_recording",
      });
    } else {
      setPopupVoice({
        status: "error",
        customMessage: result.error,
      });
    }
  }

  function addCompareSlot() {
    if (!canAddCompareSlot(slots, presets.length)) {
      return;
    }
    const presetId = nextComparePresetId(presetIds, slots);
    if (!presetId) {
      return;
    }
    const slot = createPopupSlot(2, presetId);
    setSlots((current) => [...current, slot]);
    setActiveSlotId(slot.id);
    void generateSlot(slot.id, presetId, false);
  }

  /** 移除对比槽：先停它的在途生成，再收回单槽（此前只能加不能减，只能靠重开窗复位）。 */
  function removeCompareSlot(slotId: number) {
    stopSlot(slotId);
    slotRunsRef.current.delete(slotId);
    setSlots((current) => removeCompareSlotFrom(current, slotId));
    setActiveSlotId(null);
  }

  function copySlot(slot: PopupSlot) {
    void commands.popupCopy(slot.output).then((result) => {
      if (result.status === "error") {
        setSlots((current) =>
          updateSlot(current, slot.id, (currentSlot) =>
            applySlotTerminal(currentSlot, "error", result.error),
          ),
        );
      } else {
        // 复制成功：与自动复制共用同一份 2s 回执（此前这里内联抄了一遍 markCopied）。
        // 「已复制」只说一次：按钮原位变成「已复制」，状态行不再复述（ADR-137 V4）。
        markCopied(slot.id);
      }
    });
  }

  /** 「替换原文」的结局（Rust `PopupReplaceOutcome.kind`，闭集）→ 回执文案。 */
  function replaceOutcomeText(kind: string) {
    switch (kind) {
      case "injected":
        return t("status.replaced");
      case "injected_clipboard_not_restored":
        return t("status.replaced_clipboard_not_restored");
      case "copied_target_changed":
        return t("status.replace_target_changed");
      default:
        return t("status.replace_copied_fallback");
    }
  }

  function replaceSlot(slot: PopupSlot) {
    void commands.popupReplace(slot.output).then((result) => {
      setSlots((current) =>
        updateSlot(current, slot.id, (currentSlot) => ({
          ...currentSlot,
          message:
            result.status === "ok"
              ? replaceOutcomeText(result.data.kind)
              : t("status.error_detail", { detail: vaultErrorText(result.error) }),
        })),
      );
    });
  }

  function saveSlotToNote(slot: PopupSlot) {
    const save = saveToTranslateSpace
      ? commands.saveTranslateSpaceResultToNote(slot.output)
      : commands.saveAiResultToNote(slot.output);
    void save.then((result) => {
      setSlots((current) =>
        updateSlot(current, slot.id, (currentSlot) => ({
          ...currentSlot,
          message:
            result.status === "ok"
              ? t("status.saved_note", { path: result.data.path })
              : t("status.error_detail", { detail: vaultErrorText(result.error) }),
          savedPath: result.status === "ok" ? result.data.path : currentSlot.savedPath,
        })),
      );
    });
  }

  const primary = slots[0];
  const primaryRunning = primary?.status === "running";
  const visibleSlotId = activeSlotId ?? primary?.id ?? null;
  const slotTabs = useTablistNavigation(
    slots.map((slot) => String(slot.id)),
    String(visibleSlotId ?? ""),
    (value) => setActiveSlotId(Number(value)),
  );
  // 结果区**首 token 才存在**（ADR-112 ④）：输入态只有输入卡，窗口按紧凑首帧就位；
  // 此前输入前就画出「结果」标题、空框与预设下拉，用户面对的是一块等着被填的空白。
  const resultsVisible = slotsHaveResults(slots);

  return (
    <HudSurface
      className="luna-popup"
      aria-label={t("title")}
      data-compare={slots.length > 1}
      autoGrow
      // 档位（ADR-112 ③）：回到输入态（编辑原文 / 改源文本）窗口缩回输入卡首帧，结果出现再扩。
      tier={resultsVisible ? "result" : "input"}
      // Esc 逃生通道（ADR-111 ⑤⑵）：兑现「关闭（Esc）」。输入框聚焦时同样关窗
      // （Bob/Easydict 口径）；未生成的草稿先冲刷回会话，重开即恢复。
      onDismiss={closePopup}
    >
      <HudHeader
        icon={<LanguagesIcon size={15} />}
        title={t("title")}
        // 配置胶囊（ADR-111 ② 五槽）：输入态在这里选目标语言 / 表达方式——Bob、DeepL 都在
        // 顶部常显「→ 目标语言」，此前得先生成一次才看得见预设在哪里。结果出现后各槽自带
        // 选择器，胶囊让位（一枚胶囊，不是常驻 chips 阵列，PRD-01-016）。
        context={
          !resultsVisible && primary ? (
            <PresetSelect
              className="luna-popup__preset-capsule"
              presets={presets}
              value={primary.presetId}
              onChange={(presetId) =>
                setSlots((current) =>
                  updateSlot(current, primary.id, (slot) => ({ ...slot, presetId })),
                )
              }
            />
          ) : null
        }
        actions={
          <>
            {/* 固定只在输入模式有意义：展示模式本来就不因失焦收起（PopupBlurState）。 */}
            {popupMode === "input" ? (
              <IconButton
                density="hud"
                label={pinned ? t("action.unpin") : t("action.pin")}
                icon={pinned ? <PinFilledIcon size={14} /> : <PinIcon size={14} />}
                aria-pressed={pinned}
                onClick={() => void togglePinned()}
              />
            ) : null}
            {canAddCompareSlot(slots, presets.length) && resultsVisible ? (
              <IconButton
                density="hud"
                label={t("action.add_compare")}
                icon={<PlusIcon size={15} />}
                onClick={addCompareSlot}
              />
            ) : null}
            <IconButton
              density="hud"
              label={t("action.close")}
              icon={<CloseIcon size={15} />}
              onClick={closePopup}
            />
          </>
        }
      />
      {/* i18n-key: popup:input.placeholder */}
      {popupMode === "input" && (
        <div className="luna-popup__input">
          <textarea
            ref={popupInputRef}
            value={popupIn}
            rows={2}
            placeholder={t("input.placeholder")}
            onFocus={inputEditing.onFocus}
            onPointerDown={inputEditing.onPointerDown}
            onBlur={() => {
              inputEditing.onBlur();
              // 失焦即冲刷：失焦收起走 Rust 的 PopupBlurState，那条路径看不到前端的防抖计时器。
              flushDraft();
            }}
            onChange={(e) => {
              setInput(e.target.value);
              scheduleDraftSync(e.target.value);
            }}
            onCompositionStart={() => {
              compositionRef.current = { active: true, endedAt: compositionRef.current.endedAt };
            }}
            onCompositionEnd={(e) => {
              // 记下这一次上屏发生在什么时候：WKWebView 里紧随其后的那一下 keydown 就是同一次
              // 物理按键，而它自己身上已经找不到任何"我属于输入法"的痕迹了。
              compositionRef.current = { active: false, endedAt: e.timeStamp };
            }}
            onKeyDown={(e) => {
              // Enter / Cmd+Enter / Ctrl+Enter 生成（对标 DeepL/Bob 零鼠标）；Shift+Enter / Alt+Enter
              // 换行；输入法组字中与上屏那一下只上屏候选（`popup-keys.ts`）。
              const action = classifyPopupInputKey(
                {
                  key: e.key,
                  shiftKey: e.shiftKey,
                  altKey: e.altKey,
                  metaKey: e.metaKey,
                  ctrlKey: e.ctrlKey,
                  // 从 `nativeEvent` 取：React 合成事件不带 `isComposing`，直接传 `e`
                  // 这条线索恒为 undefined（此前就是这样，等于只剩 keyCode 229 一条）。
                  isComposing: e.nativeEvent.isComposing,
                  keyCode: e.nativeEvent.keyCode,
                  timeStamp: e.timeStamp,
                },
                compositionRef.current,
              );
              // 只有真的要生成时才 `preventDefault`：`ignore` 是输入法的上屏键（吃掉它，
              // 候选词就永远上不了屏），`newline` 是 textarea 的原生换行，两者都原样放行。
              if (action !== "generate") return;
              e.preventDefault();
              generatePrimarySlot();
            }}
          />
          <div className="luna-popup__input-row">
            <span className="luna-popup__input-status" role="status">
              {inputAssistText}
            </span>
            <div className="luna-spacer" />
            <IconButton
              density="hud"
              label={t("action.screenshot")}
              icon={<CameraIcon size={15} />}
              onClick={() => void commands.popupScreenshotTranslate()}
            />
            <IconButton
              density="hud"
              className="luna-popup__mic"
              label={isPopupVoiceActive(popupVoice) ? t("action.voice_stop") : t("action.voice_start")}
              icon={isPopupVoiceActive(popupVoice) ? <StopIcon size={15} /> : <MicIcon size={15} />}
              aria-pressed={isPopupVoiceActive(popupVoice)}
              data-recording={isPopupVoiceActive(popupVoice) || undefined}
              onClick={() => void togglePopupVoiceInput()}
            />
            {/* 唯一文字主动作：生成中原位变「停止」（中断一律 `--danger`，ADR-111 ②）；
                空输入禁用——此前空着点也会打一次注定失败的往返。 */}
            {primaryRunning && primary ? (
              <button
                type="button"
                className="luna-btn luna-btn--danger luna-popup__generate"
                onClick={() => stopSlot(primary.id)}
              >
                <StopCircleIcon size={15} />
                <span>{t("action.stop")}</span>
              </button>
            ) : (
              // 一个状态一个主动作（ADR-137 V3）：有了结果，这一屏的主动作是用结果（复制 / 替换），
              // 生成退为次级——回车照样重生成（真机 2026-10-02：两颗主色按钮同屏）。
              <button
                type="button"
                className={`luna-btn ${resultsVisible ? "luna-btn--secondary" : "luna-btn--primary"} luna-popup__generate`}
                disabled={popupIn.trim() === ""}
                onClick={generatePrimarySlot}
              >
                <SparklesIcon size={15} />
                <span>{t("action.generate")}</span>
              </button>
            )}
          </div>
          {/* 语音失败落成错误卡（成因 + 修复深链 + 重试），不再借用提示位。 */}
          {voiceFailed ? (
            <FailureCard
              code={popupVoice.failureCode ?? null}
              message={popupVoice.customMessage ?? t("status.voice_error")}
              retry={{ label: t("action.voice_start"), onClick: () => void togglePopupVoiceInput() }}
            />
          ) : null}
        </div>
      )}
      {popupMode === "display" && popupSource !== null && (
        <section className="luna-popup__source" aria-label={t("source.label")}>
          <div className="luna-popup__source-head">
            <span>{t("source.label")}</span>
            <div className="luna-popup__source-actions">
              <SpeakButton
                text={popupSource}
                scope="hud:source"
                className="luna-icon-btn luna-icon-btn--hud"
                title={t("action.speak_source")}
                activeTitle={t("action.speak_stop")}
              />
              <IconButton
                density="hud"
                label={sourceExpanded ? t("source.collapse") : t("source.expand")}
                icon={<ChevronDownIcon size={14} />}
                aria-expanded={sourceExpanded}
                className="luna-popup__source-toggle"
                onClick={() => setSourceExpanded((open) => !open)}
              />
              <button type="button" className="luna-popup__source-edit" onClick={editSource}>
                <PencilIcon size={12} />
                <span>{t("source.edit")}</span>
              </button>
            </div>
          </div>
          <p className="luna-popup__source-text" data-expanded={sourceExpanded}>
            {popupSource}
          </p>
        </section>
      )}
      {resultsVisible && (
        <div className="luna-popup__slots" data-slot-count={slots.length}>
          {slots.length > 1 ? (
            /* 对比是分段切换（ADR-137 H4）：一次看一张，窗宽留给译文。 */
            <div className="luna-popup__tabs" role="tablist" aria-label={t("slot.compareLabel")}>
              {slots.map((slot) => {
                const preset = presets.find((item) => item.id === slot.presetId);
                const selected = slot.id === visibleSlotId;
                return (
                  <button
                    key={slot.id}
                    ref={slotTabs.register(String(slot.id))}
                    type="button"
                    role="tab"
                    className="luna-popup__tab"
                    aria-selected={selected}
                    tabIndex={slotTabs.tabIndexOf(String(slot.id))}
                    onKeyDown={slotTabs.onKeyDown}
                    onClick={() => setActiveSlotId(slot.id)}
                  >
                    {preset ? presetLabel(preset, t) : slot.presetId}
                  </button>
                );
              })}
            </div>
          ) : null}
          {slots
            .filter((slot) => slots.length === 1 || slot.id === visibleSlotId)
            .map((slot) => (
            <PopupResultSlot
              key={slot.id}
              presets={presets}
              slot={slot}
              copied={copiedSlotId === slot.id}
              onCopy={() => copySlot(slot)}
              onPresetChange={(presetId) => void generateSlot(slot.id, presetId, false)}
              onRegenerate={() => void generateSlot(slot.id, slot.presetId, true)}
              // 输入模式下主槽的「停止」由输入行主按钮承载；对比槽与展示模式仍在槽内。
              onStop={
                popupMode === "input" && slot.id === primary?.id ? undefined : () => stopSlot(slot.id)
              }
              onRemove={slot.id !== primary?.id ? () => removeCompareSlot(slot.id) : undefined}
              onOutputChange={(output) =>
                setSlots((current) =>
                  updateSlot(current, slot.id, (currentSlot) => ({ ...currentSlot, output })),
                )
              }
              onReplace={() => replaceSlot(slot)}
              onSaveNote={() => saveSlotToNote(slot)}
              showReplace={canReplaceSource}
            />
          ))}
        </div>
      )}
      <HudReceipt
        receipt={
          clipboardNotRestored
            ? { kind: "error", message: t("status.clipboard_not_restored") }
            : null
        }
      />
    </HudSurface>
  );
}
