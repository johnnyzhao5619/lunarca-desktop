/**
 * 录音控制 Hook（ADR-003：业务逻辑全在 Rust，前端只发命令/订阅事件/渲染）。
 *
 * - 电平：二进制 Channel + rAF 合并（复用 voice-waveform 工具，ADR-011）。
 * - 隐私铁律：RecordingEvent::Partial 仅含时间区间，无正文，不在 UI 层暴露。
 * - Archived 事件触发 luna:recording-archived 自定义事件，供 WorkspaceShell 刷新列表。
 */

import { useEffect, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import {
  commands,
  events,
  type AudioInputDeviceDto,
  type RawBytes,
  type RecordingEvent,
  type RecordingState,
  type RecordingStatus,
} from "../../ipc/bindings";
import {
  appendLevelFrames,
  decodeTrackLevels,
  LEVEL_TRACK_SYSTEM,
  type VoiceLevel,
  type VoiceLevelPayload,
} from "../voice-input/voice-waveform";
import { buildSubtitleView, updateTrackHypothesis } from "./live-subtitle";
import { acceptsBroadcastState, crossesSessionBoundary } from "./recording-state-sync";

// 全局热键桥：Rust 在 show 面板后 eval 对应钩子（PRD-04-001 / PRD-04-005 / PRD-04-040）。
// 录音与字幕各自独立钩子，互不干扰。
declare global {
  interface Window {
    __lunaRecordingHotkey?: (cmd: RecordingHotkeyCommand) => void;
    __lunaCaptionHotkey?: (cmd: RecordingHotkeyCommand) => void;
  }
}

/** 全局热键能交给 HUD 的命令：开始/停止、录音中打点。 */
export type RecordingHotkeyCommand = "toggle" | "bookmark";

/**
 * 打点热键到达时在本窗口派发的事件：打点的时刻与确认反馈归 HUD 组件那一份实现，
 * 本 hook 不复制一遍。
 */
export const RECORDING_BOOKMARK_HOTKEY_EVENT = "luna:recording-bookmark-hotkey";

/** 会话形态：录音归档 / 实时字幕（无持久化）。决定 start/dismiss 走哪条命令。 */
export type RecordingMode = "record" | "caption";

export interface UseRecordingOptions {
  /** 形态（默认 record）。字幕面板传 "caption"：不建笔记、不归档。 */
  mode?: RecordingMode;
}

// ─── 常量 ─────────────────────────────────────────────────────────────────────

const MAX_LEVEL_FRAMES = 32;
// HUD 实时滚动仅保留最近 N 行（全文经 finalize 落盘到笔记，HUD 非事实源）。
const MAX_TRANSCRIPT_LINES = 50;

// ─── 类型 ─────────────────────────────────────────────────────────────────────

/** 实时转录已定稿行（committed，永不回退；PRD-04-010）。 */
export interface TranscriptLine {
  startMs: number;
  text: string;
  /** 轨级说话人标签（PRD-04-053 L0）："self"=我 / "remote"=对方；null=单麦克风源无标签。 */
  track?: string | null;
}

/** 轨级暂定尾部；Both 的麦克风与系统轨各持一条，互不覆盖。 */
export interface HypothesisLine {
  text: string;
  /** "self"=麦克风轨 / "remote"=系统轨；null=单麦克风源。 */
  track?: string | null;
}

/**
 * 已译句界单元（一句原文 → 一条译文）。携带 `[startMs, endMs]` 覆盖区间，供字幕视图把
 * 该区间内的 committed 转录行重组为源句、与译文编织成双语 cue（PRD-04-011，不依赖逐段精确配对）。
 */
export interface TranslationEntry {
  startMs: number;
  endMs: number;
  text: string;
  /** 译文所属音轨；双源会话只与同轨原文配对。 */
  track?: string | null;
}

export interface RecordingActions {
  start: () => Promise<void>;
  stop: () => Promise<void>;
  pause: () => Promise<{ ok: true } | { error: string }>;
  resume: () => Promise<{ ok: true } | { error: string }>;
  /** 打点成功后返回会话时刻；失败时返回原因。 */
  bookmark: () => Promise<{ ms: number } | { error: string }>;
  /** 给刚打的书签补 / 改备注（PRD-04-005）。 */
  bookmarkNote: (ms: number, note: string) => Promise<{ ok: true } | { error: string }>;
  /** 麦克风静音（只影响麦克风轨；系统音频持续采集，PRD-04-003）。 */
  setMicrophoneMuted: (muted: boolean) => Promise<{ ok: true } | { error: string }>;
  /** 实时转写开关（仅录音形态；字幕形态转写恒开）。 */
  setTranscribe: (enabled: boolean) => void;
  /** 实时转译开关（两种形态均持久化，并对活动会话即时热切）。 */
  setTranslate: (enabled: boolean) => void;
  /** 现场切换转译目标外语（写设置持久化 + 当前会话热切，PRD-04-041）。 */
  setTranslateTarget: (lang: string) => void;
  /** 现场切换转写源语言（"auto" = 自动检测；写设置持久化 + 当前会话热切，PRD-04-046）。 */
  setTranscribeLang: (lang: string) => void;
  /** 把已积累的字幕（原文+译文全量）保存为 Vault 笔记（PRD-04-045）；返回新笔记路径。 */
  saveNote: () => Promise<{ path: string } | { error: string }>;
  /** 选择输入设备（写 recording.input_device 设置，下次开录生效）。 */
  setInputDevice: (name: string) => void;
  /** 采集源三选（PRD-04-052：mic / system_loopback / both；写回设置，下次开录生效）。 */
  setCaptureSource: (value: string) => void;
  /** 关闭/隐藏常驻 HUD 面板（仅非录音态可用：复位 idle + 隐藏窗口，面板 dismiss 逃生通道）。 */
  dismiss: () => Promise<void>;
}

export interface UseRecordingResult {
  state: RecordingState;
  levels: VoiceLevel[];
  /** 副轨（Both 系统音频）电平；单源会话恒空（PRD-04-052 双电平条）。 */
  systemLevels: VoiceLevel[];
  /** null = 未降级；否则为降级档位字符串（"yellow"/"orange"/"red"）。 */
  degradedTier: string | null;
  /**
   * 这次转写由谁执行、音频会不会离开设备（ADR-038 ④ 的联网指示）。
   *
   * `leavesDevice` 在**任何音频离开设备之前**就为真——它来自会话建立时 Provider
   * 自报的位置，不等服务端回话。`name` 随后由服务端补上（在此之前为 null）。
   */
  executor: { name: string | null; leavesDevice: boolean } | null;
  /**
   * 实时转写停了、但录音继续时的稳定错误码；`null` = 一切正常。
   *
   * 与 `state.status === "error"` 分开：录音本身完全正常（音频照常落盘，停录后的
   * 二次转写照常补齐权威结果）。用整个会话的失败去表达「字幕不出了」，会让用户以为
   * 这一场白录了。
   */
  transcribeStopped: string | null;
  /**
   * 云腿断了、正在静默重连窗内重试（ADR-038 ⑧）；接上或走完窗即 `false`。
   *
   * 与 `transcribeStopped` 分开：那条说「这条轨的转写到此为止」（用户要去加额度 /
   * 登录 / 改隐私档），这条说「等几秒」。共用一条的话，一次 Wi-Fi 漫游就会让 HUD
   * 弹出「转写已停止」，用户去点了一圈设置，而字幕早就回来了。
   */
  transcribeReconnecting: boolean;
  /**
   * **实时转译**停了、但转写与录音继续时的稳定错误码；`null` = 一切正常（ADR-083 ⑧）。
   *
   * 与 `transcribeStopped` 分开：那条说「字幕不出字了」，这条说「原文照常上屏，
   * 只是没有译文」——两句话对用户的含义与下一步动作都不同。完整双语仍由停录后的
   * 二次转写补齐（PRD-04-030）。
   *
   * 用户自己按的停止不会点亮它：`Cancelled` 不是结构性错误（`JobError::is_structural`），
   * 否则每一次正常停录都会弹一条「翻译已停止」。
   */
  translateStopped: string | null;
  /** 降级原因码（"high_rtf" / "window_drop"；与 degradedTier 同生命周期）。 */
  degradedReason: string | null;
  /** 实时转录已定稿行（滚动显示，PRD-04-010）。 */
  transcript: TranscriptLine[];
  /** 各轨暂定尾部（灰显；空数组 = 无暂定）。 */
  hypotheses: HypothesisLine[];
  /** 已译句界单元（按 startMs 升序；字幕视图据 [startMs,endMs] 区间重组源句配对，PRD-04-011）。 */
  translations: TranslationEntry[];
  /** 当前会话麦克风静音位（系统音频不受影响；会话起点恒为未静音）。 */
  microphoneMuted: boolean;
  /** 刚归档的那一篇的相对路径（HUD「已保存 · 打开」回执）；新会话开始即清。 */
  archivedNote: string | null;
  clearArchivedNote: () => void;
  /** 实时转写选择（录音形态镜像持久设置；字幕形态恒为 true）。 */
  transcribeEnabled: boolean;
  /** 实时转译选择（各形态镜像自身持久设置）。 */
  translateEnabled: boolean;
  /** 转译目标外语码（持久设置 realtime.translate_target_lang 的 UI 镜像，PRD-04-041）。 */
  translateTarget: string;
  /** 转写源语言（"auto" | 语种码；持久设置 realtime.transcribe_lang 的 UI 镜像，PRD-04-046）。 */
  transcribeLang: string;
  /** auto 模式下引擎检测锁定的语言码（回显「自动检测（中文）」；null = 未检测/显式指定）。 */
  detectedLang: string | null;
  /** 可用输入设备（HUD 设备选择，PRD-04-003）。 */
  devices: AudioInputDeviceDto[];
  /** 当前形态的采集源：录音读 `recording.capture_source`，字幕读 `caption.capture_source`。 */
  captureSource: string;
  /** 当前 recording.input_device 设置值（"" = 系统默认）。 */
  inputDevice: string;
  actions: RecordingActions;
}

// ─── 辅助 ─────────────────────────────────────────────────────────────────────

function createIdleState(): RecordingState {
  return {
    status: "idle",
    purpose: "record",
    session_id: null,
    note_rel: null,
    elapsed_ms: 0,
    message_key: "recording:status.idle",
    message_detail: null,
  };
}

export function isRecordingActive(status: RecordingStatus): boolean {
  return status === "recording" || status === "paused";
}

/** 会话仍占用控制权：活动态 + 已接受停止、尚未完成资源收尾。 */
export function isRecordingSessionOpen(status: RecordingStatus): boolean {
  return isRecordingActive(status) || status === "stopping";
}

// ─── Hook ─────────────────────────────────────────────────────────────────────

export function useRecording(options: UseRecordingOptions = {}): UseRecordingResult {
  const mode: RecordingMode = options.mode ?? "record";
  const [state, setState] = useState<RecordingState>(createIdleState);
  const [levels, setLevels] = useState<VoiceLevel[]>([]);
  // 副轨（Both 系统音频）电平（PRD-04-052 双电平条；单源会话恒空）。
  const [systemLevels, setSystemLevels] = useState<VoiceLevel[]>([]);
  const [degradedTier, setDegradedTier] = useState<string | null>(null);
  const [executor, setExecutor] = useState<UseRecordingResult["executor"]>(null);
  const [transcribeStopped, setTranscribeStopped] = useState<string | null>(null);
  const [translateStopped, setTranslateStopped] = useState<string | null>(null);
  const [transcribeReconnecting, setTranscribeReconnecting] = useState(false);
  const [degradedReason, setDegradedReason] = useState<string | null>(null);
  const [transcript, setTranscript] = useState<TranscriptLine[]>([]);
  const [hypotheses, setHypotheses] = useState<HypothesisLine[]>([]);
  const [translations, setTranslations] = useState<TranslationEntry[]>([]);
  const [microphoneMuted, setMicrophoneMutedState] = useState(false);
  /**
   * 刚归档的那一篇（ADR-137 H5）：HUD 据此给「已保存 · 打开」回执。Rust 归档后停留几秒
   * 再收起 HUD，回执才看得见；新会话开始即清。
   */
  const [archivedNote, setArchivedNote] = useState<string | null>(null);
  const [transcribeEnabled, setTranscribeEnabled] = useState(true);
  // 字幕形态转译默认开（核心价值即跨语言）；录音形态默认关（设置初值会覆盖）。
  const [translateEnabled, setTranslateEnabled] = useState(mode === "caption");
  const [translateTarget, setTranslateTargetState] = useState("en");
  const [transcribeLang, setTranscribeLangState] = useState("auto");
  const [detectedLang, setDetectedLang] = useState<string | null>(null);
  const [devices, setDevices] = useState<AudioInputDeviceDto[]>([]);
  const [inputDevice, setInputDeviceState] = useState("");
  const [captureSource, setCaptureSourceState] = useState(
    mode === "caption" ? "both" : "input_device",
  );

  const pendingFramesRef = useRef<VoiceLevelPayload[]>([]);
  const rafRef = useRef<number | null>(null);
  const startingRef = useRef(false);
  const stateRef = useRef(state);
  const translateEnabledRef = useRef(translateEnabled);
  // 完整字幕累积（不受显示上限约束）：仅用于「保存为笔记」（PRD-04-045）。显示态另有上限截断。
  const fullTranscriptRef = useRef<TranscriptLine[]>([]);
  const fullTranslationsRef = useRef<TranslationEntry[]>([]);

  useEffect(() => {
    stateRef.current = state;
  }, [state]);

  // 权威状态落地的**唯一入口**（H2 / M2）：会话 Channel 的 `State`、控制器的转移广播、
  // 显示时重播的快照都从这里进。跨过会话边界（结束 / 换场）即清横幅与降级档——它们绑定
  // 在会话实例上，此前只在 `start()` 清，于是「实时字幕已停」在空闲态常驻、关窗重开还挂着。
  function adoptState(next: RecordingState) {
    if (crossesSessionBoundary(stateRef.current, next)) {
      setTranscribeStopped(null);
      setTranslateStopped(null);
      setTranscribeReconnecting(false);
      setDegradedTier(null);
      setDegradedReason(null);
    }
    stateRef.current = next;
    setState(next);
  }

  // 挂载时：初始状态同步 + 订阅电平通道（复用 VoiceInputHud 同等模式，ADR-011）
  useEffect(() => {
    void commands.recordingState().then((next) => {
      if (acceptsBroadcastState(mode, next)) adoptState(next);
    });
    // 控制器状态转移的全窗广播（H2）：会话 Channel 只通向发起会话的那一扇窗，且面板隐藏
    // /锁屏期间丢掉的终态没有第二条路——此前 HUD 会永远停在「正在停止…」。录音 HUD 是常驻
    // webview（React 只挂载一次），Rust 在每次显示面板时重播一次快照，也从这里进。
    const unlistenState = events.recordingStateChanged.listen((event) => {
      if (acceptsBroadcastState(mode, event.payload)) adoptState(event.payload);
    });

    // 设备列表 + 设置初值（设备选择/转写开关，PRD-04-003/012）。
    void commands.listAudioInputDevices().then(setDevices);
    void commands.listSettings().then((result) => {
      if (result.status !== "ok") return;
      for (const spec of result.data) {
        if (spec.key === "recording.input_device" && spec.current_value.kind === "str") {
          setInputDeviceState(spec.current_value.value);
        }
        if (
          spec.key === (mode === "caption" ? "caption.capture_source" : "recording.capture_source") &&
          spec.current_value.kind === "str"
        ) {
          setCaptureSourceState(spec.current_value.value);
        }
        // 两种任务各读自身设置；字幕本身恒开，仅实时翻译偏好由 caption 设置拥有。
        if (
          mode === "record" &&
          spec.key === "realtime.transcribe_enabled" &&
          spec.current_value.kind === "bool"
        ) {
          setTranscribeEnabled(spec.current_value.value);
        }
        if (
          mode === "record" &&
          spec.key === "realtime.translate_enabled" &&
          spec.current_value.kind === "bool"
        ) {
          translateEnabledRef.current = spec.current_value.value;
          setTranslateEnabled(spec.current_value.value);
        }
        if (
          mode === "caption" &&
          spec.key === "caption.translation_enabled" &&
          spec.current_value.kind === "bool"
        ) {
          translateEnabledRef.current = spec.current_value.value;
          setTranslateEnabled(spec.current_value.value);
        }
        if (spec.key === "realtime.translate_target_lang" && spec.current_value.kind === "str") {
          setTranslateTargetState(spec.current_value.value);
        }
        // 转写源语言持久默认（"auto"/语种码；空值按 auto，PRD-04-046）。
        if (spec.key === "realtime.transcribe_lang" && spec.current_value.kind === "str") {
          setTranscribeLangState(spec.current_value.value.trim() || "auto");
        }
      }
    });

    // 字幕窗是启动即挂载的常驻 webview；从 HUD 跳到主窗设置修改声音来源后，必须经
    // Rust 的跨窗口设置事件即时回显，不能等重启或重新挂载。
    const captureSourceKey =
      mode === "caption" ? "caption.capture_source" : "recording.capture_source";
    const unlistenSettings = events.settingsChanged.listen((event) => {
      const relevant =
        event.payload.key === captureSourceKey ||
        (mode === "caption" && event.payload.key === "caption.translation_enabled");
      if (!relevant) return;
      void commands.listSettings().then((result) => {
        if (result.status !== "ok") return;
        const spec = result.data.find((item) => item.key === captureSourceKey);
        if (spec?.current_value.kind === "str") {
          setCaptureSourceState(spec.current_value.value);
        }
        if (mode === "caption") {
          const translationSpec = result.data.find(
            (item) => item.key === "caption.translation_enabled",
          );
          if (translationSpec?.current_value.kind === "bool") {
            const enabled = translationSpec.current_value.value;
            const changed = translateEnabledRef.current !== enabled;
            translateEnabledRef.current = enabled;
            setTranslateEnabled(enabled);
            // 设置中心与 HUD 是同一事实源：从任一入口切换都应同步当前字幕会话，
            // 而不是让界面显示与后端闸门分叉。ref 去重 HUD 自身写设置产生的回流事件。
            if (changed && isRecordingActive(stateRef.current.status)) {
              void commands.recordingSetTranslate(stateRef.current.session_id, enabled);
            }
          }
        }
      });
    });

    const levelChannel = new Channel<ArrayBuffer>();
    levelChannel.onmessage = (frame) => {
      pendingFramesRef.current.push(frame as VoiceLevelPayload);
      if (rafRef.current !== null) return;
      rafRef.current = window.requestAnimationFrame(() => {
        rafRef.current = null;
        // 消息带轨前缀（TDD-02 §6 双轨电平）：按轨分拣到主/副两个滚动缓冲。
        const pending = pendingFramesRef.current.splice(0).map(decodeTrackLevels);
        const primary = pending.filter((p) => p.track !== LEVEL_TRACK_SYSTEM);
        const system = pending.filter((p) => p.track === LEVEL_TRACK_SYSTEM);
        if (primary.length > 0) {
          setLevels((current) =>
            primary.reduce(
              (next, batch) => appendLevelFrames(next, batch.levels, MAX_LEVEL_FRAMES),
              current,
            ),
          );
        }
        if (system.length > 0) {
          setSystemLevels((current) =>
            system.reduce(
              (next, batch) => appendLevelFrames(next, batch.levels, MAX_LEVEL_FRAMES),
              current,
            ),
          );
        }
      });
    };
    void commands.subscribeRecordingLevels(levelChannel as unknown as Channel<RawBytes>);

    // 全局热键：toggle 空闲则开始、活动中则停止；bookmark 只在录音会话中转交 HUD 打点。
    const hotkey = (cmd: RecordingHotkeyCommand) => {
      if (cmd === "bookmark") {
        if (mode === "record" && isRecordingActive(stateRef.current.status)) {
          window.dispatchEvent(new Event(RECORDING_BOOKMARK_HOTKEY_EVENT));
        }
        return;
      }
      if (isRecordingActive(stateRef.current.status)) {
        void stop();
      } else {
        void start();
      }
    };
    if (mode === "caption") {
      window.__lunaCaptionHotkey = hotkey;
    } else {
      window.__lunaRecordingHotkey = hotkey;
    }

    return () => {
      if (mode === "caption") {
        delete window.__lunaCaptionHotkey;
      } else {
        delete window.__lunaRecordingHotkey;
      }
      if (rafRef.current !== null) {
        window.cancelAnimationFrame(rafRef.current);
      }
      void unlistenSettings.then((unlisten) => unlisten());
      void unlistenState.then((unlisten) => unlisten());
    };
  }, []);

  // ── 命令 ──────────────────────────────────────────────────────────────────

  async function start() {
    if (startingRef.current || isRecordingSessionOpen(stateRef.current.status)) return;
    startingRef.current = true;
    setArchivedNote(null);

    // 录音形态先创建归档笔记（后端 finalize 时写入转写内容）；字幕形态不建笔记（不归档）。
    let noteRel = "";
    if (mode === "record") {
      const noteResult = await commands.createNote("recording");
      if (noteResult.status === "error") {
        setState((cur) => ({
          ...cur,
          status: "error",
          message_key: "recording:error.note_create_failed",
          message_detail: noteResult.error,
        }));
        startingRef.current = false;
        return;
      }
      noteRel = noteResult.data.path;
    }

    const sessionId = crypto.randomUUID();
    // 新会话：清空上一会话的实时转录/转译显示、静音位、降级横幅与电平缓冲
    //（副轨电平尤须清——上一场 Both 的残留会让单源新会话误显第二条电平）。
    setTranscript([]);
    setHypotheses([]);
    setTranslations([]);
    fullTranscriptRef.current = [];
    fullTranslationsRef.current = [];
    setLevels([]);
    setSystemLevels([]);
    setMicrophoneMutedState(false);
    setDegradedTier(null);
    setDegradedReason(null);
    setDetectedLang(null);
    // 每次会话重置：上一场是云、这一场是本地时，残留的指示会持续误报"正在联网"。
    setExecutor(null);
    setTranscribeStopped(null);
    setTranslateStopped(null);
    setTranscribeReconnecting(false);

    const eventChannel = new Channel<RecordingEvent>();
    eventChannel.onmessage = (event) => {
      switch (event.kind) {
        case "state": {
          adoptState(event.data);
          break;
        }
        case "partial": {
          // 已定稿段 → 滚动追加（PRD-04-010 实时转录显示；committed 永不回退）。
          const line: TranscriptLine = {
            startMs: event.data.start_ms ?? 0,
            text: event.data.text,
            track: event.data.track ?? null,
          };
          fullTranscriptRef.current.push(line); // 完整累积（保存为笔记用，不截断）
          setTranscript((cur) => {
            // specta f64 → number|null（NaN 序列化语义）；时间仅作 key/排序，null 兜 0。
            const next = [...cur, line];
            return next.slice(Math.max(0, next.length - MAX_TRANSCRIPT_LINES));
          });
          break;
        }
        case "hypothesis": {
          // 暂定尾按轨全量替换：空串只清对应轨，不得抹掉仍在识别的另一输入源。
          const track = event.data.track ?? null;
          setHypotheses((cur) => updateTrackHypothesis(cur, { text: event.data.text, track }));
          break;
        }
        case "translation": {
          // 已译句界单元 → 记 [startMs,endMs] 覆盖区间 + 译文，供字幕视图重组源句配对。
          const entry: TranslationEntry = {
            startMs: event.data.start_ms ?? 0,
            endMs: event.data.end_ms ?? event.data.start_ms ?? 0,
            text: event.data.text,
            track: event.data.track ?? null,
          };
          // 完整累积（保存为笔记用，不截断）：按 startMs 去重覆盖。
          fullTranslationsRef.current = [
            ...fullTranslationsRef.current.filter(
              (e) =>
                e.startMs !== entry.startMs || (e.track ?? null) !== (entry.track ?? null),
            ),
            entry,
          ].sort(
            (a, b) => a.startMs - b.startMs || (a.track ?? "").localeCompare(b.track ?? ""),
          );
          setTranslations((cur) => {
            const next = cur.filter(
              (e) =>
                e.startMs !== entry.startMs || (e.track ?? null) !== (entry.track ?? null),
            );
            next.push(entry);
            next.sort(
              (a, b) => a.startMs - b.startMs || (a.track ?? "").localeCompare(b.track ?? ""),
            );
            // 与 transcript 同源限窗（HUD 非事实源，全文归档另有）：仅保留最近若干句。
            return next.slice(Math.max(0, next.length - MAX_TRANSCRIPT_LINES));
          });
          break;
        }
        case "degraded":
          setDegradedTier(event.data.tier);
          setDegradedReason(event.data.reason);
          break;
        case "executor":
          setExecutor({
            name: event.data.display_name,
            leavesDevice: event.data.leaves_device,
          });
          break;
        case "transcribe_stopped":
          setTranscribeStopped(event.data.code);
          // 转写彻底停了就不再是「重连中」——两条横幅同时挂着会互相矛盾。
          setTranscribeReconnecting(false);
          break;
        // 转译整条腿停了（结构性失败，本会话不再逐句提交）。转写与录音不受影响，
        // 故**不**动 `transcribeStopped` / `transcribeReconnecting` 那两位。
        case "translate_stopped":
          setTranslateStopped(event.data.code);
          break;
        case "transcribe_reconnecting":
          setTranscribeReconnecting(event.data.active);
          break;
        case "transcribe_lang_detected":
          // auto 模式引擎检测锁定回显（PRD-04-046）：null = 清除（切换/待重新检测）。
          setDetectedLang(event.data.lang);
          break;
        case "archived":
          setArchivedNote(event.data.note_rel);
          // 通知 WorkspaceShell 刷新录音列表
          window.dispatchEvent(
            new CustomEvent("luna:recording-archived", {
              detail: event.data.note_rel,
            }),
          );
          // 录音结束归档 → 自隐藏常驻 HUD 由 **Rust 侧**收口（run_session 归档后 hide_recording_hud）：
          // 前端 JS window.hide() 对非激活 NSPanel 受 capability 约束无效，故不在此 hide。
          break;
        case "failed":
          setState((cur) => ({
            ...cur,
            status: "error",
            message_key: `recording:error.${event.data.code.toLowerCase()}`,
          }));
          break;
      }
    };

    let result: Awaited<ReturnType<typeof commands.captionStart>>;
    try {
      result =
        mode === "caption"
          ? await commands.captionStart(
              sessionId,
              eventChannel,
            )
          : await commands.recordingStart(sessionId, noteRel, eventChannel);
    } catch (error) {
      setState((cur) => ({
        ...cur,
        status: "error",
        message_key: "recording:error.start_failed",
        message_detail: error instanceof Error ? error.message : String(error),
      }));
      return;
    } finally {
      startingRef.current = false;
    }
    if (result.status === "error") {
      setState((cur) => ({
        ...cur,
        status: "error",
        message_key: "recording:error.start_failed",
        message_detail: result.error,
      }));
    } else {
      setState(result.data);
      stateRef.current = result.data;
    }
  }

  async function stop() {
    if (!isRecordingActive(stateRef.current.status)) return;
    const result = await commands.recordingStop(stateRef.current.session_id);
    if (result.status === "error") {
      setState((cur) => ({
        ...cur,
        status: "error",
        message_key: "recording:error.stop_failed",
        message_detail: result.error,
      }));
    } else {
      // 停止命令直接返回控制器在同一临界区写入的权威 Stopping 快照；即便本窗口的
      // Channel 状态事件稍后到达或投递失败，按钮也会立刻进入不可重复点击的收尾态。
      setState(result.data);
      stateRef.current = result.data;
    }
  }

  async function pause() {
    const result = await commands.recordingPause(stateRef.current.session_id);
    if (result.status === "ok") {
      setState(result.data);
      stateRef.current = result.data;
      return { ok: true } as const;
    }
    return { error: result.error };
  }

  async function resume() {
    const result = await commands.recordingResume(stateRef.current.session_id);
    if (result.status === "ok") {
      setState(result.data);
      stateRef.current = result.data;
      return { ok: true } as const;
    }
    return { error: result.error };
  }

  async function bookmark(): Promise<{ ms: number } | { error: string }> {
    const ms = stateRef.current.elapsed_ms ?? 0;
    try {
      const result = await commands.recordingBookmark(stateRef.current.session_id, ms, null);
      return result.status === "ok" ? { ms } : { error: result.error };
    } catch (error) {
      return { error: error instanceof Error ? error.message : String(error) };
    }
  }

  async function bookmarkNote(
    ms: number,
    note: string,
  ): Promise<{ ok: true } | { error: string }> {
    const text = note.trim();
    if (text.length === 0) {
      return { ok: true };
    }
    try {
      const result = await commands.recordingBookmarkNote(stateRef.current.session_id, ms, text);
      return result.status === "ok" ? { ok: true } : { error: result.error };
    } catch (error) {
      return { error: error instanceof Error ? error.message : String(error) };
    }
  }

  async function setMicrophoneMuted(next: boolean) {
    const result = await commands.recordingSetMicrophoneMuted(stateRef.current.session_id, next);
    if (result.status === "error") {
      return { error: result.error };
    }
    setMicrophoneMutedState(next);
    return { ok: true } as const;
  }

  function setTranscribe(enabled: boolean) {
    if (mode === "caption") return; // 字幕会话的领域不变量：转写恒开。
    setTranscribeEnabled(enabled);
    void commands.setSetting("realtime.transcribe_enabled", { kind: "bool", value: enabled });
    if (isRecordingActive(stateRef.current.status)) {
      void commands.recordingSetTranscribe(stateRef.current.session_id, enabled);
    }
  }

  function setTranslate(enabled: boolean) {
    translateEnabledRef.current = enabled;
    setTranslateEnabled(enabled);
    void commands.setSetting(
      mode === "caption" ? "caption.translation_enabled" : "realtime.translate_enabled",
      { kind: "bool", value: enabled },
    );
    if (isRecordingActive(stateRef.current.status)) {
      void commands.recordingSetTranslate(stateRef.current.session_id, enabled);
    }
  }

  function setInputDevice(name: string) {
    setInputDeviceState(name);
    void commands.setSetting("recording.input_device", { kind: "str", value: name });
  }

  function setCaptureSource(value: string) {
    setCaptureSourceState(value);
    void commands.setSetting(mode === "caption" ? "caption.capture_source" : "recording.capture_source", {
      kind: "str",
      value,
    });
  }

  function setTranslateTarget(lang: string) {
    setTranslateTargetState(lang);
    // 持久默认（下次会话）+ 当前会话热切（下一句界即按新目标解析，PRD-04-041）。
    void commands.setSetting("realtime.translate_target_lang", { kind: "str", value: lang });
    if (isRecordingActive(stateRef.current.status)) {
      void commands.recordingSetTranslateTarget(stateRef.current.session_id, lang);
    }
  }

  function setTranscribeLang(lang: string) {
    setTranscribeLangState(lang);
    // 切换立即清检测回显（显式指定无需回显；回 auto 待引擎重新检测再回报，PRD-04-046）。
    setDetectedLang(null);
    // 持久默认（下次会话）+ 当前会话热切（当前解码窗之后即生效）。
    void commands.setSetting("realtime.transcribe_lang", { kind: "str", value: lang });
    if (isRecordingActive(stateRef.current.status)) {
      void commands.recordingSetTranscribeLang(stateRef.current.session_id, lang);
    }
  }

  // 保存为笔记（PRD-04-045）：用**完整累积**（非显示截断）按句配对成双语行，交 Rust 落盘。
  async function saveNote(): Promise<{ path: string } | { error: string }> {
    const cues = buildSubtitleView(
      fullTranscriptRef.current,
      [],
      fullTranslationsRef.current,
      Number.MAX_SAFE_INTEGER,
    ).cues;
    const lines = cues
      .map((cue) => ({ source: cue.source, translation: cue.translation }))
      .filter((line) => line.source.trim() !== "" || (line.translation ?? "").trim() !== "");
    if (lines.length === 0) return { error: "empty" };
    const result = await commands.captionSaveNote(lines);
    return result.status === "ok" ? { path: result.data } : { error: result.error };
  }

  // 关闭面板（经 **Rust 命令**隐藏——JS hide 对 NSPanel 受 ACL 限制无效）。
  //
  // **录音形态：关闭恒可用，且关窗 ≠ 中断**（ADR-111 已知代价 ③）。会话进行中只藏窗
  // ——本地状态一个都不动（Rust 的 `dismiss_idle` 对活动会话本就是 no-op，窗口重开即
  // 回到现场；录音中的可见性由状态栏「录音中」持续态指示兜底，PRD-15-060）。
  // 非会话态才复位 idle（清 error/失败残留）。
  //
  // **字幕形态刻意保持「仅非会话态可关」**：字幕窗本身就是产物——藏起字幕还继续听，
  // 没有任何受益者；会话中的退出动作是「停止」。这与录音不同：录音的产物（音频/转写）
  // 在后台继续累积，藏窗有明确收益。
  async function dismiss() {
    if (isRecordingSessionOpen(stateRef.current.status)) {
      if (mode === "caption") return;
      await commands.recordingHudDismiss();
      return;
    }
    const idle = createIdleState();
    setState(idle);
    stateRef.current = idle;
    // 关窗即清上一场的横幅与降级档（M2）：重开窗口不恢复上一场的横幅。
    setTranscribeStopped(null);
    setTranslateStopped(null);
    setTranscribeReconnecting(false);
    setDegradedTier(null);
    setDegradedReason(null);
    setTranscript([]);
    setHypotheses([]);
    setTranslations([]);
    startingRef.current = false;
    if (mode === "caption") {
      await commands.captionDismiss();
    } else {
      await commands.recordingHudDismiss();
    }
  }

  return {
    state,
    levels,
    systemLevels,
    degradedTier,
    executor,
    transcribeStopped,
    translateStopped,
    transcribeReconnecting,
    degradedReason,
    transcript,
    hypotheses,
    translations,
    microphoneMuted,
    archivedNote,
    clearArchivedNote: () => setArchivedNote(null),
    transcribeEnabled,
    translateEnabled,
    translateTarget,
    transcribeLang,
    detectedLang,
    devices,
    captureSource,
    inputDevice,
    actions: {
      start,
      stop,
      pause,
      resume,
      bookmark,
      bookmarkNote,
      setMicrophoneMuted,
      setTranscribe,
      setTranslate,
      setTranslateTarget,
      setTranscribeLang,
      saveNote,
      setInputDevice,
      setCaptureSource,
      dismiss,
    },
  };
}
