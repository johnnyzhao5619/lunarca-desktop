/**
 * 语音输入 HUD 的业务层（PRD-02；ADR-003 前端零业务——这里只发命令、订阅事件、持会话态）。
 *
 * 2026-09-07 从 `VoiceInputHud.tsx`（1381 行）拆出：会话生命周期、事件订阅、电平通道、
 * 热键桥、模型缺失修复、落地、设置读写全在这一层；组件只剩形态渲染（与录音 HUD 的
 * `use-recording.ts` / `RecordingHud.tsx` 同一分工）。拆分**零行为变更**——每一段都是
 * 原样迁入，注释里记录的真机坑一并带过来。
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import {
  commands,
  events,
  type LandingMode,
  type RawBytes,
  type SettingValueDto,
  type VoiceInputEvent,
  type VoiceInputMode,
  type VoiceInputState,
} from "../../ipc/bindings";
import { useModels, useSettings } from "../settings/use-settings";
import { formatShortcut } from "../../lib/shortcut-display";
import { usePersistentVoiceMode } from "./voice-modes";
import { pushLevelFrames, type VoiceLevel, type VoiceLevelPayload } from "./voice-waveform";
import { goTo } from "../../lib/navigate";
import {
  applyVoiceModePatch,
  isVoiceSessionActive,
  nextLivePartial,
  nextEditedVoiceOutput,
  nextTranscriptFeedback,
  shouldResetVoiceLevels,
  type LivePartial,
  type TranscriptFeedback,
} from "./voice-hud-state";

declare global {
  interface Window {
    __lunaVoiceHotkey?: (action: "pressed" | "released") => void;
  }
}

const MAX_LEVEL_FRAMES = 36;

export type VoiceUplink = {
  name: string | null;
  leavesDevice: boolean;
};

function createInitialState(): VoiceInputState {
  return {
    status: "idle",
    landing_mode: "inject",
    input_mode: "hold",
    auto_refine: true,
    translate_enabled: false,
    translate_target_lang: "en",
    message_key: "voice:status.idle",
    message_detail: null,
    result_pending: false,
  };
}

function nextSessionId() {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

export function useVoiceInput() {
  const { t } = useTranslation("voice-input");
  const [state, setState] = useState<VoiceInputState>(() => createInitialState());
  const [levels, setLevels] = useState<VoiceLevel[]>([]);
  // 联网指示：三扇窗共用 `HudUplinkDot`（文案单一事实源在 recording ns，ADR-111 ⑤⑴）。
  const [uplink, setUplink] = useState<VoiceUplink | null>(null);

  const { models, progressById, install, assign } = useModels();
  const { specs, refresh } = useSettings();
  const { allModes, activeModeId, setActiveModeId } = usePersistentVoiceMode();
  const [isDownloading, setIsDownloading] = useState(false);
  const [downloadError, setDownloadError] = useState<string | null>(null);
  const [transcriptFeedback, setTranscriptFeedback] = useState<TranscriptFeedback | null>(null);
  // 按住说话期间的实时识别反馈（PRD-02-004）：committed 累计 + 灰显暂定尾。
  const [livePartial, setLivePartial] = useState<LivePartial | null>(null);
  // 结果编辑稿（PRD-02-022）：识别结果可先修正再落地；每次新结果重置。
  const [editedText, setEditedText] = useState("");
  const previousTranscriptFeedbackRef = useRef<TranscriptFeedback | null>(null);
  const [elapsedMs, setElapsedMs] = useState(0);
  const [configOpen, setConfigOpen] = useState(false);
  /**
   * 落地回执（ADR-137 V5）：每次 Rust 报告落地成功就换一个序号，回执线据此出现一次、
   * 淡出后清掉。用序号而不是比对文案键——「是不是刚落地」由那次命令的返回 / `landed`
   * 事件说了算，前端不按 `message_key` 猜。
   */
  const [landingReceipt, setLandingReceipt] = useState<number | null>(null);
  const landingReceiptSeqRef = useRef(0);
  const announceLanding = () => setLandingReceipt(++landingReceiptSeqRef.current);
  /** 最近一次 `failed` 事件的稳定错误码：出错卡据它查失败注册表给修复动作。 */
  const [failureCode, setFailureCode] = useState<string | null>(null);
  /**
   * 「存为速记后 AI 提炼」（PRD-06-058 C′）：设置 `voice_input.quick_note_refine` 的当前值，
   * 与其它 HUD 内设置一样从已加载的规格快照里读（`null` = 规格未到，不渲染成"关"去误导）。
   * 此前只在打开设置面板时另发一次 `listSettings`：首次展开那一瞬这枚开关"迟到"出现，
   * 面板抖一下、连带自动扩容改窗口高度（R-22）。它不进 Rust 的会话态：提炼发生在落盘之后、
   * 由 Rust 按同一个设置键决定，HUD 只是把这枚开关放到用户手边。
   */
  const quickNoteRefine = useMemo(() => {
    const value = specs.find((s) => s.key === "voice_input.quick_note_refine")?.current_value;
    return value?.kind === "bool" ? value.value : null;
  }, [specs]);

  useEffect(() => {
    // 状态更新函数必须是纯函数：此前把「记下上一份反馈」这个副作用写在 updater 里，
    // React 在开发态会把 updater 调两遍——第二遍看到"上一份 === 这一份"，于是保留了旧的
    // 空编辑稿。真机表现：识别出了文字、「优化结果」框却是空的，还挂着「已编辑」。
    const previous = previousTranscriptFeedbackRef.current;
    previousTranscriptFeedbackRef.current = transcriptFeedback;
    setEditedText((current) => nextEditedVoiceOutput(current, previous, transcriptFeedback));
  }, [transcriptFeedback]);

  const downloadableAsrModel = useMemo(() => {
    return models.find(
      (m) => m.declared_capabilities.includes("transcribe") && m.status === "downloadable",
    );
  }, [models]);
  const pendingFramesRef = useRef<VoiceLevelPayload[]>([]);
  const rafRef = useRef<number | null>(null);
  const activeRef = useRef(false);
  const stateRef = useRef(state);
  const activeSessionIdRef = useRef<string | null>(null);
  const pendingControlSessionIdRef = useRef<string | null>(null);
  const startPromiseRef = useRef<Promise<VoiceInputState | null> | null>(null);
  const sessionStartedAtRef = useRef<number | null>(null);

  // 固定位置（设置为唯一事实源，内部键；HUD 的「⋯」里切换）。
  const pinned = useMemo(() => {
    const spec = specs.find((s) => s.key === "voice_input.hud_pinned");
    return spec?.current_value.kind === "bool" ? spec.current_value.value : false;
  }, [specs]);
  // 全局快捷键提示（单一来源：settings registry + shortcut-display 展示规约）。
  const voiceShortcut = useMemo(() => {
    const spec = specs.find((s) => s.key === "shortcuts.voice_input");
    return formatShortcut(spec?.current_value.kind === "str" ? spec.current_value.value : "");
  }, [specs]);

  useEffect(() => {
    stateRef.current = state;
    activeRef.current = isVoiceSessionActive(state.status);
  }, [state]);

  useEffect(() => {
    if (!isVoiceSessionActive(state.status)) {
      sessionStartedAtRef.current = null;
      setElapsedMs(0);
      return;
    }
    if (sessionStartedAtRef.current === null) {
      sessionStartedAtRef.current = Date.now();
    }
    const tick = () => {
      if (sessionStartedAtRef.current !== null) {
        setElapsedMs(Date.now() - sessionStartedAtRef.current);
      }
    };
    tick();
    const interval = window.setInterval(tick, 1000);
    return () => window.clearInterval(interval);
  }, [state.status]);

  useEffect(() => {
    void commands.voiceInputState().then(setState);

    const levelChannel = new Channel<ArrayBuffer>();
    levelChannel.onmessage = (frame) => {
      pendingFramesRef.current.push(frame);
      if (rafRef.current !== null) {
        return;
      }
      rafRef.current = window.requestAnimationFrame(() => {
        rafRef.current = null;
        const pending = pendingFramesRef.current.splice(0);
        setLevels((current) =>
          pending.reduce((next, batch) => pushLevelFrames(next, batch, MAX_LEVEL_FRAMES), current),
        );
      });
    };
    void commands.voiceInputSubscribeLevels(levelChannel as unknown as Channel<RawBytes>);

    window.__lunaVoiceHotkey = (action) => {
      const current = stateRef.current;
      if (current.input_mode === "toggle") {
        if (action === "pressed") {
          if (activeRef.current || startPromiseRef.current) {
            void stopVoice(activeSessionIdRef.current);
          } else {
            void startVoice();
          }
        }
        return;
      }
      if (action === "pressed") {
        void startVoice();
      } else {
        void stopVoice();
      }
    };

    // **每次唤起都重读一次会话状态**（`voice-input://hud-opened`，TDD-07 §2.3）。
    //
    // 上面那句 `voiceInputState()` 只在**挂载时**跑一次，而这扇 HUD 是**预热长驻**窗口：
    // 第二次之后的唤起复用同一个已挂载的 webview，effect 不再执行，于是 Rust 侧
    // `prepare_session` 刚设好的落点预设**永远到不了这一屏**。真机（2026-08-26）表现：
    // 从「添加内容 → 语音速记」进去，入口标注写着「存到「速记」」，HUD 摘要却是
    // 「注入光标」——正是 PRD-07-069 明令禁止的"入口标注去向 ≠ 实际落点"。
    // Rust 侧一直在发这个事件（`voice.rs` 那句注释与顺序断言都写着"据此重读"），
    // 只是前端从来没接。载荷刻意为空：它只是"去重新问一次"的信号，真值恒以 Rust 为准。
    const opened = events.voiceInputHudOpened.listen(() => {
      void commands.voiceInputState().then(setState);
    });
    return () => {
      delete window.__lunaVoiceHotkey;
      void opened.then((unlisten) => unlisten());
      if (rafRef.current !== null) {
        window.cancelAnimationFrame(rafRef.current);
      }
    };
  }, []);

  function syncSessionLifecycle(nextState: VoiceInputState) {
    stateRef.current = nextState;
    activeRef.current = isVoiceSessionActive(nextState.status);
    if (!activeRef.current) {
      activeSessionIdRef.current = null;
      pendingControlSessionIdRef.current = null;
    }
  }

  function showSessionError(error: string) {
    setState((current) => ({
      ...current,
      status: "error",
      message_key: "voice:status.error",
      message_detail: error,
    }));
  }

  async function startVoice() {
    if (activeRef.current || startPromiseRef.current) {
      return;
    }
    pendingFramesRef.current = [];
    setLevels([]);
    setTranscriptFeedback(null);
    setLivePartial(null);
    setConfigOpen(false);
    setLandingReceipt(null);
    setFailureCode(null);
    const sessionId = nextSessionId();
    activeSessionIdRef.current = sessionId;
    // 每次会话重置：上一次走云、这一次走本地时，残留的指示会持续误报「正在联网」。
    setUplink(null);
    const eventChannel = new Channel<VoiceInputEvent>();
    eventChannel.onmessage = (event) => {
      if (shouldResetVoiceLevels(event)) {
        pendingFramesRef.current = [];
        setLevels([]);
      }
      setTranscriptFeedback((current) => nextTranscriptFeedback(current, event));
      setLivePartial((current) => nextLivePartial(current, event));
      switch (event.kind) {
        case "state":
          setState(event.data);
          syncSessionLifecycle(event.data);
          break;
        case "failed":
          setFailureCode(event.data.code);
          setState((current) => {
            const next: VoiceInputState = {
              ...current,
              status: "error",
              message_key: `errors:${event.data.code.toLowerCase()}`,
            };
            syncSessionLifecycle(next);
            return next;
          });
          break;
        case "executor":
          // 联网指示（ADR-038 ④ / PRD-04-060）：语音输入是**最高频**的麦克风路径，
          // 单次几秒但一天几十次，音频离开设备这件事在这里同样必须可见。
          setUplink({
            name: event.data.display_name,
            leavesDevice: event.data.leaves_device,
          });
          break;
        case "landed":
          // 非审阅路径的自动落地（弹窗输入框等专用 sink）：同样给一次回执。
          announceLanding();
          break;
        case "partial":
        case "field_input":
        case "transcript":
          break;
      }
    };
    const startPromise = commands
      .voiceInputStart(sessionId, activeModeId, eventChannel)
      .then((result) => {
        if (result.status === "ok") {
          setState(result.data);
          syncSessionLifecycle(result.data);
          return result.data;
        }
        setState((current) => {
          const next: VoiceInputState = {
            ...current,
            status: "error",
            message_key: "voice:status.error",
            message_detail: result.error,
          };
          syncSessionLifecycle(next);
          return next;
        });
        return null;
      });
    startPromiseRef.current = startPromise;
    const startedState = await startPromise.finally(() => {
      if (startPromiseRef.current === startPromise) {
        startPromiseRef.current = null;
      }
    });
    if (pendingControlSessionIdRef.current === sessionId) {
      pendingControlSessionIdRef.current = null;
      if (startedState && isVoiceSessionActive(startedState.status)) {
        await stopVoiceNow(sessionId);
      }
    }
  }

  async function stopVoice(sessionId = activeSessionIdRef.current) {
    const pendingStart = startPromiseRef.current;
    if (pendingStart && sessionId) {
      pendingControlSessionIdRef.current = sessionId;
      await pendingStart;
      return;
    }
    await stopVoiceNow(sessionId);
  }

  async function stopVoiceNow(sessionId: string | null) {
    const result = await commands.voiceInputStop(sessionId);
    if (result.status === "error") {
      showSessionError(result.error);
    }
  }

  async function cancelVoice() {
    const sessionId = activeSessionIdRef.current;
    const pendingStart = startPromiseRef.current;
    if (pendingStart && sessionId) {
      pendingControlSessionIdRef.current = sessionId;
      await pendingStart;
      if (!activeRef.current && activeSessionIdRef.current !== sessionId) {
        return;
      }
    }
    const result = await commands.voiceInputCancel(sessionId);
    setTranscriptFeedback(null);
    setLivePartial(null);
    setLevels([]);
    if (result.status === "error") {
      showSessionError(result.error);
    }
  }

  async function retryVoice() {
    await cancelVoice();
    await startVoice();
  }

  async function handleAutoFix() {
    if (!downloadableAsrModel) return;
    setIsDownloading(true);
    setDownloadError(null);
    try {
      const result = await install(downloadableAsrModel.id);
      if (!result) {
        setDownloadError(t("error.install_failed"));
        setIsDownloading(false);
        return;
      }
      await assign("realtime_asr", downloadableAsrModel.id);
      setIsDownloading(false);
      void startVoice();
    } catch {
      setDownloadError(t("error.install_failed"));
      setIsDownloading(false);
    }
  }

  async function closeVoicePanel() {
    const result = await commands.voiceInputClose();
    if (result.status === "error") {
      showSessionError(result.error);
    }
  }

  async function landVoiceOutput(landingMode: LandingMode) {
    // 编辑稿与识别结果不同才作为覆盖稿传递（后端空白稿自动忽略）。
    const override =
      transcriptFeedback && editedText.trim() !== transcriptFeedback.finalText.trim()
        ? editedText
        : null;
    const result = await commands.voiceInputLand(landingMode, override);
    if (result.status === "ok") {
      setState(result.data);
      syncSessionLifecycle(result.data);
      if (result.data.status !== "error") announceLanding();
      return;
    }
    showSessionError(result.error);
  }

  async function persistVoiceSetting(key: string, value: SettingValueDto) {
    const result = await commands.setSetting(key, value);
    if (result.status === "error") {
      showSessionError(result.error);
      return false;
    }
    return true;
  }

  async function updateVoiceMode(
    patch: Parameters<typeof applyVoiceModePatch>[1],
    setting?: { key: string; value: SettingValueDto },
  ) {
    if (setting && !(await persistVoiceSetting(setting.key, setting.value))) {
      return;
    }
    const payload = applyVoiceModePatch(stateRef.current, patch);
    const result = await commands.voiceInputSetMode(
      payload.landingMode,
      payload.inputMode,
      payload.autoRefine,
      payload.translateEnabled,
      payload.translateTargetLang,
    );
    if (result.status === "ok") {
      setState(result.data);
      syncSessionLifecycle(result.data);
    } else {
      showSessionError(result.error);
    }
  }

  async function setLandingMode(landingMode: LandingMode) {
    await updateVoiceMode(
      { landingMode },
      { key: "voice_input.landing_mode", value: { kind: "str", value: landingMode } },
    );
  }

  async function setInputMode(inputMode: VoiceInputMode) {
    await updateVoiceMode(
      { inputMode },
      { key: "voice_input.input_mode", value: { kind: "str", value: inputMode } },
    );
  }

  async function setAutoRefine(autoRefine: boolean) {
    await updateVoiceMode(
      { autoRefine },
      { key: "voice_input.auto_refine", value: { kind: "bool", value: autoRefine } },
    );
  }

  async function setQuickNoteRefine(enabled: boolean) {
    if (
      await persistVoiceSetting("voice_input.quick_note_refine", { kind: "bool", value: enabled })
    ) {
      await refresh();
    }
  }

  async function setTranslateEnabled(translateEnabled: boolean) {
    await updateVoiceMode(
      { translateEnabled },
      { key: "voice_input.translate_enabled", value: { kind: "bool", value: translateEnabled } },
    );
  }

  async function setTranslateTargetLang(translateTargetLang: string) {
    await updateVoiceMode(
      { translateTargetLang },
      {
        key: "voice_input.translate_target_lang",
        value: { kind: "str", value: translateTargetLang },
      },
    );
  }

  async function togglePinned() {
    const result = await commands.voiceInputSetHudPinned(!pinned);
    if (result.status === "error") {
      showSessionError(result.error);
      return;
    }
    await refresh();
  }

  /** 这份结果已存成速记的路径（Rust 落地后放在 `message_detail`）；`null` = 还没存。 */
  const savedNotePath =
    state.message_key === "voice:status.saved_note" && state.message_detail
      ? state.message_detail
      : null;
  function openSavedNote() {
    if (savedNotePath) goTo(`quick-notes://${savedNotePath}`);
  }

  return {
    state,
    levels,
    uplink,
    transcriptFeedback,
    livePartial,
    editedText,
    setEditedText,
    elapsedMs,
    configOpen,
    setConfigOpen,
    quickNoteRefine,
    downloadableAsrModel,
    progressById,
    isDownloading,
    downloadError,
    allModes,
    activeModeId,
    setActiveModeId,
    pinned,
    landingReceipt,
    clearLandingReceipt: () => setLandingReceipt(null),
    failureCode,
    voiceShortcut,
    savedNotePath,
    actions: {
      startVoice,
      stopVoice,
      cancelVoice,
      retryVoice,
      handleAutoFix,
      closeVoicePanel,
      landVoiceOutput,
      setLandingMode,
      setInputMode,
      setAutoRefine,
      setQuickNoteRefine,
      setTranslateEnabled,
      setTranslateTargetLang,
      togglePinned,
      openSavedNote,
    },
  };
}

export type VoiceInputController = ReturnType<typeof useVoiceInput>;
