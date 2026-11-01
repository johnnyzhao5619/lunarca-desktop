/**
 * 录音 HUD 面板（ADR-015：非激活 NSPanel，失焦不收起）。
 * 业务逻辑：useRecording hook；本组件只渲染+调用 actions。
 *
 * 形态（PRD-04-003 修订 2026-09-07，ADR-112 ④）：**纯录音态是一条控制条**——录音点与计时
 * 紧邻操作键、同处控制条（Loom 式），空闲不显示计时；字幕画布只在「实时转写」开着时存在；
 * 配置摘要挪到窗口栏之下（此前在停止键下方，录音中改语言要越过破坏性的停止键）；同一时刻
 * 至多一条非阻断横幅（`recording-banner.ts`）。字幕之外仅保留录音基础控制——静音/暂停/
 * 书签/停止；转写/转译/目标语并入摘要下的轻控件条。
 *
 * 档位（ADR-112 ③，2026-09-08）：bar / card / panel 由 `HudSurface.tier` 交给 Rust `hud_set_tier`，
 * 收起设置或关掉实时转写时窗口缩回该档首帧高。
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { SelectBox } from "../../components/ui/SelectBox";
import { IconButton } from "../../components/ui/IconButton";
import { useTranslation } from "react-i18next";
import {
  RECORDING_BOOKMARK_HOTKEY_EVENT,
  isRecordingActive,
  isRecordingSessionOpen,
  useRecording,
} from "./use-recording";
import { formatClock } from "../../lib/format-duration";
import { compactVoiceLevels, voiceLevelBucket } from "../voice-input/voice-waveform";
import type { RecordingStatus } from "../../ipc/bindings";
import {
  BookmarkIcon,
  CheckSquareIcon,
  ChevronDownIcon,
  CloseIcon,
  DiscIcon,
  MicIcon,
  MicOffIcon,
  PauseIcon,
  PlayIcon,
  SettingsIcon,
  SquareIcon,
  StopIcon,
} from "../../components/ui/Icon";
import { HudSurface } from "../../components/ui/HudSurface";
import { HudHeader } from "../../components/ui/HudHeader";
import { HudReceipt, type HudReceiptValue } from "../../components/ui/HudReceipt";
import { goTo } from "../../lib/navigate";
import { usePanelTextEditing } from "../../hooks/use-panel-text-editing";
import { buildSubtitleView } from "./live-subtitle";
import { LiveSubtitleView } from "./LiveSubtitleView";
import { SubtitleLangFlow } from "./SubtitleLangFlow";
import { useTranslationMirror } from "./use-translation-mirror";
import { pickRecordingBanner } from "./recording-banner";
import { bookmarkAutoDismissMs } from "./bookmark-toast-policy";
import { vaultErrorText } from "../../lib/vault-error";
import { isImeKeyEvent } from "../../lib/ime-key-event";

type Translate = (key: string, options?: Record<string, unknown>) => string;

// ─── 辅助 ─────────────────────────────────────────────────────────────────────

/**
 * 状态键 → 人话。兜底一律走 `defaultValue`：i18next 未命中时返回的是 key 本身
 * （非空串），`t(key) || 兜底` 永远不生效——未知错误码会把 `error.SOME_CODE` 原样糊到界面上。
 */
function statusMessage(
  messageKey: string | null | undefined,
  status: RecordingStatus,
  t: Translate,
): string {
  if (!messageKey) {
    return status === "error" ? t("status.error") : t("status.idle");
  }
  // i18n key 形如 "recording:status.recording" → 取后半段
  const key = messageKey.startsWith("recording:")
    ? messageKey.slice("recording:".length)
    : messageKey;
  // 错误码形如 "errors:model_missing"
  if (messageKey.startsWith("errors:")) {
    const code = messageKey.slice("errors:".length);
    return t(`error.${code}`, { defaultValue: t("status.error") });
  }
  return t(key, { defaultValue: t("status.idle") });
}

/** 控制条里内嵌的小波形：只是「在收音」的证据，不是频谱。 */
const RECORDING_METER_FRAMES = 16;
// 卡片档只给最近两条 cue（ADR-137 H5）：录音 HUD 是控制条，字幕是「它在听」的证据；
// 要回看就去实时字幕窗或录音详情。
const RECORDING_MAX_CUES = 2;

// ─── 组件 ─────────────────────────────────────────────────────────────────────

export function RecordingHud() {
  const { t } = useTranslation("recording");
  const { t: tCommon } = useTranslation("common");
  const { t: tSettings } = useTranslation("settings");
  const {
    state,
    levels,
    systemLevels,
    degradedTier,
    degradedReason,
    executor,
    transcribeStopped,
    translateStopped,
    transcribeReconnecting,
    transcript,
    hypotheses,
    translations,
    microphoneMuted,
    archivedNote,
    clearArchivedNote,
    transcribeEnabled,
    translateEnabled,
    translateTarget,
    transcribeLang,
    detectedLang,
    devices,
    captureSource,
    inputDevice,
    actions,
  } = useRecording();

  const bars = useMemo(() => compactVoiceLevels(levels, RECORDING_METER_FRAMES), [levels]);
  const systemBars = useMemo(
    () => compactVoiceLevels(systemLevels, RECORDING_METER_FRAMES),
    [systemLevels],
  );
  // 双电平条（PRD-04-052「两者」）：Both 会话显示「我/对方」两行；数据驱动兜底——
  // 副轨真有电平帧也显示（设置在会话中被改走时不误隐藏正在跳动的副轨）。
  const dualMeter = captureSource === "both" || systemLevels.length > 0;
  const subtitleView = useMemo(
    () => buildSubtitleView(transcript, hypotheses, translations, RECORDING_MAX_CUES),
    [transcript, hypotheses, translations],
  );

  const active = isRecordingActive(state.status);
  const sessionOpen = isRecordingSessionOpen(state.status);
  const elapsed = formatClock(state.elapsed_ms ?? 0);
  const message = statusMessage(state.message_key, state.status, t);
  const translateActive = translateEnabled && transcribeEnabled;

  // 译文独立窗（PRD-04-044 录音扩展，共用 Hook）：开启时 HUD 仅显示转写原文，
  // 译文交由 live-caption-translation 镜像窗大字呈现（双屏/给对方看/会议演示）。
  const mirror = useTranslationMirror(
    "record",
    transcript,
    hypotheses,
    translations,
    active && translateActive,
    state.session_id,
  );
  const translationDetached = translateActive && mirror.separate;

  // 设置始终默认折叠：空闲态也先呈现稳定的控制条，设备/语言按需展开。
  const [settingsOpen, setSettingsOpen] = useState(false);
  const captureSourceLabel = tSettings(`option.recording.capture_source.${captureSource}`);
  const processingLabel = translateActive
    ? translationDetached
      ? t("label.translationDetached")
      : t("settings.transcribeAndTranslate")
    : transcribeEnabled
      ? t("option.transcribe")
      : t("settings.recordOnly");
  const configSummary = t("settings.summary", {
    source: captureSourceLabel,
    processing: processingLabel,
  });

  // 同一时刻至多一条非阻断横幅：按优先级取最高的一条，其余折进它的悬停说明。
  const { shown: banner, folded: foldedBanners } = pickRecordingBanner({
    transcribeStopped,
    transcribeReconnecting,
    translateStopped,
    translateEnabled,
    degradedTier,
    degradedReason,
  });
  const bannerText = (key: string, fallbackKey: string) =>
    t(key, { defaultValue: t(fallbackKey) });

  // 书签确认反馈（PRD-04-005）：点击后显示「已打点」+ 时刻 + 一格备注输入；空着且没在编辑
  // 才 6s 自动收起（`bookmark-toast-policy.ts`）；Enter 提交、Esc 只失焦、✕ 才关。打点本身
  // 在点击那一刻已经落到 Rust，备注是随后追加——两步都在录音中完成，不用等停录再翻笔记。
  const [bookmarkedAt, setBookmarkedAt] = useState<number | null>(null);
  const [bookmarkNote, setBookmarkNote] = useState("");
  const [bookmarkEditingNow, setBookmarkEditingNow] = useState(false);
  const [bookmarkBusy, setBookmarkBusy] = useState(false);
  const bookmarkPendingRef = useRef(false);
  const [bookmarkError, setBookmarkError] = useState<string | null>(null);
  const [controlBusy, setControlBusy] = useState<"mute" | "pause" | "resume" | null>(null);
  const [controlError, setControlError] = useState<string | null>(null);
  const bookmarkTimerRef = useRef<number | null>(null);
  const bookmarkInputRef = useRef<HTMLInputElement>(null);
  const bookmarkEditing = usePanelTextEditing();
  useEffect(
    () => () => {
      if (bookmarkTimerRef.current !== null) window.clearTimeout(bookmarkTimerRef.current);
    },
    [],
  );

  function clearBookmarkTimer() {
    if (bookmarkTimerRef.current !== null) window.clearTimeout(bookmarkTimerRef.current);
    bookmarkTimerRef.current = null;
  }

  function dismissBookmark() {
    clearBookmarkTimer();
    setBookmarkedAt(null);
    setBookmarkNote("");
    setBookmarkError(null);
  }

  // 策略变化（打点 / 输入 / 聚焦 / 失焦）即重算倒计时：策略说不收就不收。
  useEffect(() => {
    clearBookmarkTimer();
    if (bookmarkedAt === null || bookmarkError !== null) return;
    const delay = bookmarkAutoDismissMs({ note: bookmarkNote, editing: bookmarkEditingNow });
    if (delay === null) return;
    bookmarkTimerRef.current = window.setTimeout(() => {
      bookmarkTimerRef.current = null;
      setBookmarkedAt(null);
      setBookmarkNote("");
    }, delay);
    return clearBookmarkTimer;
  }, [bookmarkedAt, bookmarkNote, bookmarkEditingNow, bookmarkError]);

  async function onBookmark() {
    if (bookmarkPendingRef.current) return;
    bookmarkPendingRef.current = true;
    setBookmarkBusy(true);
    setBookmarkError(null);
    try {
      const result = await actions.bookmark();
      if ("error" in result) {
        setBookmarkError(vaultErrorText(result.error));
        return;
      }
      setBookmarkedAt(result.ms);
      setBookmarkNote("");
    } finally {
      bookmarkPendingRef.current = false;
      setBookmarkBusy(false);
    }
  }

  // 全局打点热键与控制条上的打点按钮走同一个处理：时刻、落盘、确认反馈只有这一份。
  const onBookmarkRef = useRef(onBookmark);
  onBookmarkRef.current = onBookmark;
  useEffect(() => {
    const onHotkey = () => void onBookmarkRef.current();
    window.addEventListener(RECORDING_BOOKMARK_HOTKEY_EVENT, onHotkey);
    return () => {
      window.removeEventListener(RECORDING_BOOKMARK_HOTKEY_EVENT, onHotkey);
    };
  }, []);

  async function submitBookmarkNote() {
    if (bookmarkedAt === null || bookmarkNote.trim().length === 0) {
      dismissBookmark();
      return;
    }
    setBookmarkBusy(true);
    setBookmarkError(null);
    const result = await actions.bookmarkNote(bookmarkedAt, bookmarkNote);
    setBookmarkBusy(false);
    if ("error" in result) {
      setBookmarkError(vaultErrorText(result.error));
      return;
    }
    dismissBookmark();
  }

  async function runControl(
    control: "mute" | "pause" | "resume",
    action: () => Promise<{ ok: true } | { error: string }>,
  ) {
    if (controlBusy !== null) return;
    setControlBusy(control);
    setControlError(null);
    try {
      const result = await action();
      if ("error" in result) {
        setControlError(vaultErrorText(result.error));
      }
    } catch (error) {
      setControlError(vaultErrorText(error instanceof Error ? error.message : String(error)));
    } finally {
      setControlBusy(null);
    }
  }

  // 状态行身份位（ADR-137 V2）：会话开着时是「● 计时」（录音中脉冲红点 / 暂停竖条）——红点是
  // 这扇窗唯一的红色（V3）；空闲时是唯一的主动作「开始录音」。
  const clock = sessionOpen ? (
    <span className="luna-rec-hud__clock">
      {state.status === "recording" && (
        <span className="luna-rec-hud__rec-dot" aria-hidden="true" />
      )}
      {state.status !== "recording" && (
        <span className="luna-rec-hud__pause-icon" aria-hidden="true" />
      )}
      <span
        className="luna-rec-hud__timer"
        aria-label={t("aria.elapsed", { time: elapsed })}
        aria-live="off"
      >
        <span aria-hidden="true">{elapsed}</span>
      </span>
    </span>
  ) : null;

  const lead =
    clock ?? (
      <button
        type="button"
        className="luna-btn luna-btn--primary luna-rec-hud__start"
        onClick={() => void actions.start()}
      >
        <MicIcon size={15} />
        {t("action.start")}
      </button>
    );

  const meterBars = (levelBars: typeof bars) =>
    levelBars.map((level, index) => (
      <span
        key={index}
        className="luna-hud-meter__bar"
        data-active={level.peak > 0 || level.rms > 0}
        data-level={voiceLevelBucket(level).toString()}
      />
    ));

  // 这一行的内容只说一件事（ADR-137 V4）：录音中是内嵌波形（Both 双轨两行「我 / 对方」），
  // 暂停 / 正在保存 / 出错是一句话，空闲是当前配置（点开即设置）。
  const lane =
    state.status === "recording" ? (
      dualMeter ? (
        <div className="luna-rec-hud__meters" aria-hidden="true" role="presentation">
          <div className="luna-rec-hud__meter" data-track="self">
            <span className="luna-rec-hud__meter-label">{tCommon("track.self")}</span>
            {meterBars(bars)}
          </div>
          <div className="luna-rec-hud__meter" data-track="remote">
            <span className="luna-rec-hud__meter-label">{tCommon("track.remote")}</span>
            {meterBars(systemBars)}
          </div>
        </div>
      ) : (
        <div className="luna-rec-hud__meter" aria-hidden="true" role="presentation">
          {meterBars(bars)}
        </div>
      )
    ) : sessionOpen || state.status === "error" || state.status === "mic_busy" ? (
      <span className="luna-rec-hud__lane-text" data-tone={state.status === "error" ? "danger" : undefined}>
        {message}
      </span>
    ) : (
      <button
        type="button"
        className="luna-rec-hud__config-summary"
        aria-expanded={settingsOpen}
        onClick={() => setSettingsOpen((open) => !open)}
      >
        <span>{configSummary}</span>
        <ChevronDownIcon size={13} />
      </button>
    );

  // 「停止」= 结束并保存，不是破坏性动作：中性强调（ADR-137 V3，红色只留给录音指示）。
  const stopButton = (
    <button
      type="button"
      className="luna-btn luna-btn--secondary luna-rec-hud__stop"
      onClick={() => void actions.stop()}
    >
      <StopIcon size={13} />
      {t("action.stop")}
    </button>
  );

  // 会话中的控制：每个状态一个主动作 + 至多两个次要动作（ADR-137 V3）。
  const controls =
    state.status === "recording" ? (
      <>
        <IconButton
          density="hud"
          label={t("action.pause")}
          icon={<PauseIcon size={14} />}
          disabled={controlBusy !== null}
          onClick={() => void runControl("pause", actions.pause)}
        />
        {/* 静音常驻（只作用于麦克风轨；回环单源本就没有麦克风可静，PRD-04-052）——常用操作
            不进二级菜单（真机 2026-10-02 反馈）。 */}
        {captureSource !== "system_loopback" ? (
          <IconButton
            density="hud"
            label={microphoneMuted ? t("action.unmute") : t("action.mute")}
            icon={microphoneMuted ? <MicOffIcon size={14} /> : <MicIcon size={14} />}
            aria-pressed={microphoneMuted}
            disabled={controlBusy !== null}
            onClick={() =>
              void runControl("mute", () => actions.setMicrophoneMuted(!microphoneMuted))
            }
          />
        ) : null}
        <IconButton
          density="hud"
          label={t("action.bookmark")}
          icon={<BookmarkIcon size={14} />}
          disabled={bookmarkBusy}
          onClick={() => void onBookmark()}
        />
        {stopButton}
      </>
    ) : state.status === "paused" ? (
      <>
        <button
          type="button"
          className="luna-btn luna-btn--primary"
          onClick={() => void runControl("resume", actions.resume)}
          disabled={controlBusy !== null}
        >
          <PlayIcon size={13} />
          {t("action.resume")}
        </button>
        {stopButton}
      </>
    ) : null;


  // 回执线（ADR-137 V5）：一次只出一条——没做成的优先，其次刚做成的。
  const receipt: HudReceiptValue | null = controlError
    ? { kind: "error", message: t("error.control_failed"), detail: controlError }
    : bookmarkError
      ? { kind: "error", message: t("bookmark.failed"), detail: bookmarkError }
      : state.status === "error"
        ? { kind: "error", message, detail: state.message_detail }
        : archivedNote && !sessionOpen
          ? {
              kind: "success",
              message: t("status.saved"),
              action: {
                label: t("action.openRecording"),
                onSelect: () => goTo(`recording://${archivedNote}`),
              },
            }
          : null;

  return (
    <HudSurface
      className="luna-rec-hud"
      aria-label={t("title")}
      data-status={state.status}
      autoGrow
      // 档位（ADR-112 ③ / PRD-04-003 修订）：纯录音 = 控制条；开实时转写 = 带最近两条字幕；
      // 设置展开 = 面板。收起设置或关掉实时转写时窗口缩回该档（真机 D 反馈：此前只涨不缩）。
      tier={settingsOpen ? "panel" : transcribeEnabled ? "card" : "bar"}
      // Esc 逃生通道（ADR-111 ⑤⑵）：dismiss 经 stateRef 自守卫，录音/暂停中只藏窗不中断
      // （「关闭恒在、关窗不停会话」，ADR-111 已知代价 ③）。设置开着先收设置（V8 退一层）。
      onDismiss={() => {
        if (settingsOpen) {
          setSettingsOpen(false);
          return;
        }
        void actions.dismiss();
      }}
    >
      {/* 读屏播报范围收窄到状态一句（此前 aria-live 挂在整个根上，字幕每来一段都打断朗读）。 */}
      <span className="luna-visually-hidden" role="status">
        {message}
      </span>

      <HudHeader
        icon={<DiscIcon size={15} />}
        lead={lead}
        title={t("title")}
        uplink={executor ? { ...executor, active: sessionOpen } : null}
        lane={lane}
        actions={
          <>
            {controls}
            {/* 录音设置（面板档）：空闲行的配置摘要是同一个开关；会话中由这枚图标打开，
                可现场切换转写 / 转译 / 语言（PRD-04-041）。不再藏进「⋯」——菜单只会把这条
                一行高的控制条撑大。 */}
            {sessionOpen ? (
              <IconButton
                density="hud"
                label={t("action.settings")}
                icon={<SettingsIcon size={14} />}
                aria-pressed={settingsOpen}
                onClick={() => setSettingsOpen((open) => !open)}
              />
            ) : null}
            {/* 关闭恒在，关窗 ≠ 中断（ADR-111 已知代价 ③）：录音中点关闭只藏窗，
                会话照常、状态栏持续态指示兜底（PRD-15-060）；重开即回到现场。 */}
            <IconButton
              density="hud"
              className="luna-rec-hud__close"
              label={t("action.closePanel")}
              icon={<CloseIcon size={14} />}
              onClick={() => void actions.dismiss()}
            />
          </>
        }
      />

      {/* 非阻断横幅（PRD-04-061 / ADR-083 ⑧ / ADR-038 ⑧）：录音**继续**，只是实时字幕
          或译文停了、或负载降级。四类文案各自独立（说的是不同的后果），但同一时刻只上屏
          一条；被折叠的几条列在 title 里。 */}
      {banner && (
        <aside
          className="luna-rec-hud__banner"
          role="status"
          data-tier={banner.tier}
          title={
            foldedBanners.length > 0
              ? foldedBanners.map((b) => bannerText(b.key, b.fallbackKey)).join("\n")
              : undefined
          }
        >
          <span className="luna-rec-hud__flat-marker" aria-hidden="true" />
          <span>{bannerText(banner.key, banner.fallbackKey)}</span>
          {foldedBanners.length > 0 && (
            <small className="luna-rec-hud__banner-more">+{foldedBanners.length}</small>
          )}
        </aside>
      )}

      {/* 设置（面板档，ADR-137 H5）：采集源、麦克风只在空闲可改；转写 / 转译 / 语言可录音中
          现场切换（PRD-04-041）。由空闲行的配置摘要或「⋯ → 录音设置」打开，Esc 先收它。 */}
      {settingsOpen && (
        <div className="luna-rec-hud__options" role="group" aria-label={t("aria.settings")}>
          <p className="luna-rec-hud__options-summary">{configSummary}</p>
          {!active && (
            <label className="luna-rec-hud__device">
              <span>{t("option.captureSource")}</span>
              {/* 采集源三选（PRD-04-052）：标签与设置页共用 settings ns（单一标签源，DRY）。 */}
              <SelectBox
                className="luna-rec-hud__device-select"
                value={captureSource}
                onChange={(event) => actions.setCaptureSource(event.currentTarget.value)}
                ariaLabel={t("option.captureSource")}
              >
                {["input_device", "system_loopback", "both"].map((value) => (
                  <option key={value} value={value}>
                    {tSettings(`option.recording.capture_source.${value}`)}
                  </option>
                ))}
              </SelectBox>
            </label>
          )}
          {!active && captureSource !== "system_loopback" && (
            <label className="luna-rec-hud__device">
              <span>{t("option.device")}</span>
              <SelectBox
                className="luna-rec-hud__device-select"
                value={inputDevice}
                onChange={(event) => actions.setInputDevice(event.currentTarget.value)}
                ariaLabel={t("option.device")}
              >
                <option value="">{t("option.deviceDefault")}</option>
                {devices.map((device) => (
                  <option key={device.name} value={device.name}>
                    {device.name}
                  </option>
                ))}
              </SelectBox>
            </label>
          )}
          <button
            type="button"
            className="luna-hud-toggle"
            aria-pressed={transcribeEnabled}
            onClick={() => actions.setTranscribe(!transcribeEnabled)}
          >
            {transcribeEnabled ? <CheckSquareIcon size={14} /> : <SquareIcon size={14} />}
            <span>{t("option.transcribe")}</span>
          </button>
          {/* 实时转译（PRD-04-011，TDD-08）：依赖实时转写（committed 段是翻译输入源） */}
          <button
            type="button"
            className="luna-hud-toggle"
            aria-pressed={translateEnabled}
            disabled={!transcribeEnabled}
            title={transcribeEnabled ? undefined : t("option.translateNeedsTranscribe")}
            onClick={() => actions.setTranslate(!translateEnabled)}
          >
            {translateActive ? <CheckSquareIcon size={14} /> : <SquareIcon size={14} />}
            <span>{t("option.translate")}</span>
          </button>
          {/* 语言流条（PRD-04-046，Chrome Live Caption 对标）：转写源语言（默认自动检测，
              引擎锁定后回显）→ 转译目标外语（PRD-04-041，仅转译开启时显示） */}
          <label className="luna-rec-hud__device">
            <span>{t("option.language")}</span>
            <SubtitleLangFlow
              transcribeLang={captureSource === "both" ? "auto" : transcribeLang}
              detectedLang={detectedLang}
              onTranscribeLangChange={actions.setTranscribeLang}
              transcribeDisabled={!transcribeEnabled || captureSource === "both"}
              translateActive={translateActive}
              translateTarget={translateTarget}
              onTranslateTargetChange={actions.setTranslateTarget}
              sourceAriaLabel={t("option.transcribeLang")}
              targetAriaLabel={t("option.translateTarget")}
            />
            {captureSource === "both" ? (
              <small className="luna-rec-hud__dual-lang-note">{t("option.dualAuto")}</small>
            ) : null}
          </label>
          {/* 译文独立窗（PRD-04-044 录音扩展）：译文移交镜像窗大字呈现，HUD 保持紧凑仅显原文 */}
          {translateActive && (
            <button
              type="button"
              className="luna-hud-toggle"
              aria-pressed={mirror.separate}
              onClick={() => mirror.setSeparate(!mirror.separate)}
              title={t("option.separateTranslationHint")}
            >
              {mirror.separate ? <CheckSquareIcon size={14} /> : <SquareIcon size={14} />}
              <span>{t("option.separateTranslation")}</span>
            </button>
          )}
        </div>
      )}

      {/* 最近两条字幕只在「实时转写」开着时存在（ADR-112 ④）；会话中首句到达前给单行等待
          占位，空闲态给引导。字幕滚动不进读屏 live 区（announce=false）。 */}
      {transcribeEnabled && !settingsOpen && (
        <section className="luna-rec-hud__live-text" data-no-drag>
          <LiveSubtitleView
            view={subtitleView}
            showSource={transcribeEnabled}
            showTranslation={translateEnabled && !mirror.separate}
            variant="hud"
            announce={false}
            emptyLabel={sessionOpen ? t("panel.waitingForSpeech") : t("panel.transcriptEmpty")}
            sourceAriaLabel={t("aria.transcript")}
            trackLabels={{ self: tCommon("track.self"), remote: tCommon("track.remote") }}
          />
        </section>
      )}

      {/* 书签回执（PRD-04-005）：打点在点击那一刻已落到 Rust；这一行给时刻 + 一格备注。
          空着且没在编辑才自动收起（`bookmark-toast-policy.ts`）。 */}
      {bookmarkedAt !== null && (
        <div className="luna-rec-hud__bookmark-toast" role="status">
          <BookmarkIcon size={13} />
          <span>{t("bookmark.added", { time: formatClock(bookmarkedAt) })}</span>
          {/* WKWebView 点击不给焦点（见 use-note-selection 的键盘契约）：mousedown 上
              preventDefault + focus，否则输入框拿不到键盘。 */}
          <input
            ref={bookmarkInputRef}
            className="luna-rec-hud__bookmark-note"
            type="text"
            value={bookmarkNote}
            placeholder={t("bookmark.notePlaceholder")}
            aria-label={t("bookmark.noteLabel")}
            maxLength={120}
            disabled={bookmarkBusy}
            onMouseDown={(event) => {
              event.preventDefault();
              bookmarkEditing.onPointerDown();
              event.currentTarget.focus({ preventScroll: true });
            }}
            onFocus={() => {
              setBookmarkEditingNow(true);
              bookmarkEditing.onFocus();
            }}
            onBlur={() => {
              setBookmarkEditingNow(false);
              bookmarkEditing.onBlur();
            }}
            onChange={(event) => setBookmarkNote(event.target.value)}
            onKeyDown={(event) => {
              if (isImeKeyEvent(event)) return;
              if (event.key === "Enter") {
                event.preventDefault();
                void submitBookmarkNote();
              } else if (event.key === "Escape") {
                // 只失焦，不清空：打了一半的备注不能被一次 Esc 吃掉；✕ 才是关。
                event.preventDefault();
                event.currentTarget.blur();
              }
            }}
          />
          <IconButton
            density="sm"
            label={t("bookmark.dismiss")}
            icon={<CloseIcon size={13} />}
            onClick={dismissBookmark}
          />
        </div>
      )}

      <HudReceipt receipt={receipt} onExpire={clearArchivedNote} />
    </HudSurface>
  );
}
