/**
 * AI 语音输入 HUD（PRD-02-018 / ADR-112 / ADR-137）——**只有一张实时表面**，按内容长三档：
 *
 * - **胶囊**（空闲 / 说话 / 处理）：一行 `[说话圆钮] [提示 · 波形 + 一行实时文字 · 扫光 +
 *   阶段] [计时] [联网点] [⋯] [✕]`。状态只在这一行说一次（ADR-137 V4）。
 * - **卡片**（识别结果 / 出错 / 关闭确认）：胶囊行 + 一块内容。结果只有一个主动作
 *   （默认去处，⌘↵）；其余去处与「重说」进「⋯」（⌘K）。出错一定带一个修复动作。
 * - **面板**（配置）：胶囊行 + 设置，由「⋯ → 语音设置」进入，Esc 收起；不持久化——
 *   此前的「标准面板」把实时表面又做了一遍（大计时、空闲说明句、第二颗主按钮）。
 *
 * 业务全在 Rust（ADR-003）。**本文件只渲染**：会话、事件、命令与设置读写在
 * `use-voice-input.ts`（与录音 HUD 的 `use-recording.ts` 同一分工）。
 */

import { useMemo, useRef, useState } from "react";
import { SelectBox } from "../../components/ui/SelectBox";
import { formatClock } from "../../lib/format-duration";
import { IconButton } from "../../components/ui/IconButton";
import { cssVariables } from "../../lib/css-variables";
import { HudSurface } from "../../components/ui/HudSurface";
import { HudHeader } from "../../components/ui/HudHeader";
import { HudReceipt, type HudReceiptValue } from "../../components/ui/HudReceipt";
import type { LandingMode, VoiceRefineMode } from "../../ipc/bindings";
import { useTranslation } from "react-i18next";
import { formatBytes } from "@lunarca/editor-core/format-bytes";
import {
  langLabelKey,
  useSettingOptions,
  VOICE_INPUT_TARGET_SETTING,
} from "../settings/language-options";
import { keyToMessage } from "./voice-message";
import {
  ArrowClockwiseIcon,
  CheckSquareIcon,
  CloseIcon,
  CopyIcon,
  MicIcon,
  NotebookIcon,
  PinFilledIcon,
  PinIcon,
  SettingsIcon,
  SparklesIcon,
  SquareIcon,
  StopIcon,
  StickyNoteIcon,
} from "@lunarca/editor-core/Icon";
import { HudActionMenu, type HudActionMenuItem } from "../../components/ui/HudActionMenu";
import { compactVoiceLevels, voiceLevelBucket } from "./voice-waveform";
import {
  buildVoicePrimaryAction,
  estimateTextRows,
  isVoiceSessionActive,
  voiceLandingActions,
  voiceFeedbackLabelKey,
  voiceStatusTone,
} from "./voice-hud-state";
import { useVoiceInput } from "./use-voice-input";
import { usePanelTextEditing } from "../../hooks/use-panel-text-editing";
import { formatShortcut } from "@lunarca/editor-core/shortcut-display";
import { failureGuidance } from "../../i18n/failure";

/** 胶囊里那段波形的柱数：一行里它只是「在听」的证据，不是频谱。 */
const VOICE_METER_FRAMES = 12;

function landingIcon(mode: LandingMode, size: number) {
  if (mode === "clipboard") return <CopyIcon size={size} />;
  if (mode === "quick_note") return <NotebookIcon size={size} />;
  return <MicIcon size={size} />;
}

export function VoiceInputHud() {
  const { t, i18n } = useTranslation("voice-input");
  const {
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
    clearLandingReceipt,
    failureCode,
    voiceShortcut,
    savedNotePath,
    actions,
  } = useVoiceInput();
  const pointerStartedRef = useRef(false);
  // 结果框编辑期让面板给输入法候选窗让路（TDD-05 §5.1.1）——改识别错的中文正是在这里打字。
  const resultEditing = usePanelTextEditing();
  const [closeConfirmOpen, setCloseConfirmOpen] = useState(false);

  const message = keyToMessage(state.message_key, state.status, t);
  const primaryAction = buildVoicePrimaryAction(state);
  // 圆钮的名字承载实际快捷键（PRD-02-015）：空闲时它也是这一行的提示语。未绑定退化为「开始说话」。
  const primaryLabel =
    primaryAction.kind === "start" && voiceShortcut
      ? t(state.input_mode === "hold" ? "action.start_hold" : "action.start_toggle", {
          shortcut: voiceShortcut,
        })
      : t(primaryAction.labelKey);
  const statusTone = voiceStatusTone(state.status);
  const activeMode = allModes.find((mode) => mode.id === activeModeId) ?? allModes[0];
  const modeLabel = (label: string) =>
    label.startsWith("voice-input:") ? t(label.replace("voice-input:", "")) : label;
  const bars = useMemo(() => compactVoiceLevels(levels, VOICE_METER_FRAMES), [levels]);
  const sessionActive = isVoiceSessionActive(state.status);
  // 目标语言候选取自设置规格（Rust 注册表单一事实源）+ 兜底并入当前值（防历史值丢失显示）。
  const targetOptions = useSettingOptions(VOICE_INPUT_TARGET_SETTING);
  // 落点候选同源注册表（规格未载入前兜底当前值，防空下拉）。
  const landingOptionValues = useSettingOptions("voice_input.landing_mode");
  const landingOptions =
    landingOptionValues.length > 0 ? landingOptionValues : [state.landing_mode];
  const translateTargets = useMemo(() => {
    const current = state.translate_target_lang;
    if (current && !targetOptions.includes(current)) {
      return [...targetOptions, current];
    }
    return targetOptions;
  }, [state.translate_target_lang, targetOptions]);
  const landingActions = useMemo(
    () => voiceLandingActions(state.landing_mode),
    [state.landing_mode],
  );
  const primaryLanding = landingActions.find((action) => action.isDefault) ?? landingActions[0];
  // 关掉会丢东西 = 会话还在跑，或有一份还没落地的结果（判定在 Rust，`result_pending`）。
  const closeWouldDiscard = sessionActive || state.result_pending;
  const modelFix = state.message_key === "errors:model_missing" && downloadableAsrModel ? downloadableAsrModel : null;
  const failed = statusTone === "danger";

  function requestClose() {
    if (closeConfirmOpen) {
      setCloseConfirmOpen(false);
      return;
    }
    // Esc 退一层（ADR-137 V8）：设置开着先收设置。
    if (configOpen) {
      setConfigOpen(false);
      return;
    }
    if (closeWouldDiscard) {
      setCloseConfirmOpen(true);
      return;
    }
    void actions.closeVoicePanel();
  }

  async function discardAndClose() {
    setCloseConfirmOpen(false);
    await actions.cancelVoice();
    await actions.closeVoicePanel();
  }

  function handlePrimaryPointerDown(event: React.PointerEvent<HTMLButtonElement>) {
    if (primaryAction.kind === "start" && state.input_mode === "hold") {
      // 指针捕获：按住期间手指/鼠标滑出按钮边界不误触停止（松开才结束，PRD-02-017）。
      event.currentTarget.setPointerCapture(event.pointerId);
      pointerStartedRef.current = true;
      void actions.startVoice();
    }
  }

  function handlePrimaryPointerUp() {
    if (pointerStartedRef.current) {
      pointerStartedRef.current = false;
      void actions.stopVoice();
    }
  }

  function handlePrimaryClick() {
    if (primaryAction.kind === "processing") return;
    if (primaryAction.kind === "start") {
      if (state.input_mode === "toggle") void actions.startVoice();
      return;
    }
    void actions.stopVoice();
  }

  function landPrimary() {
    if (savedNotePath) {
      actions.openSavedNote();
      return;
    }
    if (primaryLanding) void actions.landVoiceOutput(primaryLanding.mode);
  }

  // 说话圆钮占身份位（它本身就是一枚麦克风）。开始 = 主色；「完成」是停止采集，用中性强调
  // ——红色只留给录音指示与不可撤销的放弃（ADR-137 V3）。有结果时圆钮让位给结果标识：
  // 那时这一屏的主动作是「落到哪儿」，不是再说一句。
  const lead = transcriptFeedback ? (
    <span className="luna-voice__badge" aria-hidden="true">
      <SparklesIcon size={15} />
    </span>
  ) : (
    <button
      type="button"
      className="luna-voice__orb"
      data-kind={primaryAction.kind}
      aria-label={primaryLabel}
      title={primaryLabel}
      disabled={primaryAction.disabled}
      onPointerDown={handlePrimaryPointerDown}
      onPointerUp={handlePrimaryPointerUp}
      onPointerCancel={handlePrimaryPointerUp}
      onClick={handlePrimaryClick}
    >
      {primaryAction.kind === "stop" ? <StopIcon size={14} /> : <MicIcon size={16} />}
    </button>
  );

  const meter = (
    <div className="luna-voice__meter" aria-hidden="true">
      {bars.map((level, index) => (
        <span
          key={`${index}-${level.peak}-${level.rms}`}
          className="luna-hud-meter__bar"
          data-active={level.peak > 0 || level.rms > 0}
          data-level={voiceLevelBucket(level).toString()}
        />
      ))}
    </div>
  );

  // 这一行的内容只说一件事：此刻在发生什么。
  const lane = transcriptFeedback ? (
    <span className="luna-voice__lane-text">
      {t(voiceFeedbackLabelKey(transcriptFeedback))}
      {editedText.trim() !== transcriptFeedback.finalText.trim() && (
        <small className="luna-voice__edited-badge">{t("preview.edited")}</small>
      )}
    </span>
  ) : statusTone === "processing" ? (
    /* 处理阶段（识别 / 优化 / 翻译，PRD-02-016）：扫光就是进度条，阶段字就在它上面。 */
    <span className="luna-voice__scan" role="progressbar" aria-label={message}>
      {message}
    </span>
  ) : statusTone === "live" ? (
    <>
      {meter}
      {/* 实时文字单行贴尾：容器右对齐、溢出裁左边，看到的永远是最新那几个字。 */}
      <span className="luna-voice__live" aria-live="polite">
        <span>
          {livePartial?.text}
          {livePartial?.hypothesis ? (
            <span className="luna-voice__live-hypothesis">
              {livePartial.text ? " " : ""}
              {livePartial.hypothesis}
            </span>
          ) : null}
        </span>
      </span>
      <span className="luna-voice__clock">{formatClock(elapsedMs)}</span>
    </>
  ) : failed ? (
    <span className="luna-voice__lane-text" data-tone="danger">
      {t("status_label.danger")}
    </span>
  ) : statusTone === "success" ? (
    <span className="luna-voice__lane-text">{message}</span>
  ) : (
    <>
      <span className="luna-voice__lane-text" data-tone="muted">
        {primaryLabel}
      </span>
      {/* 落点只是默认高亮：识别后先审阅、点了才写入（PRD-02-015 / 020）——胶囊没有地方摆
          一句说明，承诺写在它的提示里，结果卡里那颗主按钮再兑现一次。 */}
      <span
        className="luna-hud-context-badge luna-voice__destination"
        title={t("landing.review_first", { destination: t(`landing.${state.landing_mode}`) })}
      >
        {t(`landing.${state.landing_mode}`)}
      </span>
    </>
  );

  // 「⋯」只在有结果时出现，装的是低频的另一个去处与重说（ADR-137 V3）。固定位置与语音设置
  // 是常用操作，常驻成两枚图标：此前它们在二级菜单里，而胶囊只有一行高——每开一次菜单整扇
  // 窗都被撑大（真机 2026-10-02）。卡片档有结果区，菜单就落在它上面，不撑窗。
  const menuItems: HudActionMenuItem[] = [
    ...(transcriptFeedback && !savedNotePath
      ? landingActions
          .filter((action) => action.mode !== primaryLanding?.mode)
          .map((action) => ({
            id: action.mode,
            label: t(action.labelKey),
            icon: landingIcon(action.mode, 14),
            onSelect: () => void actions.landVoiceOutput(action.mode),
          }))
      : []),
    ...(transcriptFeedback
      ? [
          {
            id: "retry",
            label: savedNotePath ? t("landing.another") : t("action.retry"),
            icon: <ArrowClockwiseIcon size={14} />,
            onSelect: () => void actions.retryVoice(),
          },
        ]
      : []),
  ];

  // 回执线（ADR-137 V5）：落地成功一次、出错常驻，一次只出一条。
  const receipt: HudReceiptValue | null = failed
    ? modelFix && !isDownloading
      ? {
          kind: "error",
          message,
          detail: t("error.model_missing_fix_tip", {
            size: formatBytes(modelFix.est_on_disk_bytes, i18n.language),
          }),
          action: { label: t("action.download_and_fix"), onSelect: () => void actions.handleAutoFix() },
        }
      : modelFix
        ? null
        : {
            kind: "error",
            message,
            detail: state.message_detail,
            code: failureCode,
            // 注册表有修复深链就用它（回执线自取）；没有的错至少能重说一次——不留没有出路的
            // 错误态（ADR-112 ② ⑶）。
            action: failureGuidance(failureCode)
              ? undefined
              : { label: t("action.retry"), onSelect: () => void actions.retryVoice() },
          }
    : downloadError
      ? {
          kind: "error",
          message: downloadError,
          action: { label: t("action.download_and_fix"), onSelect: () => void actions.handleAutoFix() },
        }
      : landingReceipt !== null
        ? { kind: "success", message }
        : null;

  const downloadProgress =
    modelFix && isDownloading
      ? (() => {
          const progress = progressById[modelFix.id];
          const downloaded = progress?.downloaded_bytes ?? 0;
          const total = progress?.total_bytes ?? 0;
          const percent = progress && total > 0 ? Math.min(100, Math.round((downloaded / total) * 100)) : 0;
          return (
            <div className="luna-voice__download" role="status">
              <div className="luna-voice__download-labels">
                <span>
                  {t("models.installing")} {modelFix.name} · {percent}%
                </span>
                <span>
                  {formatBytes(downloaded, i18n.language)} / {formatBytes(total, i18n.language)}
                </span>
              </div>
              <div className="luna-voice__progress-bar">
                <div
                  className="luna-voice__progress-fill"
                  style={cssVariables({ "--progress-inline-size": `${percent}%` })}
                />
              </div>
            </div>
          );
        })()
      : null;

  const resultRows = estimateTextRows(editedText, { unitsPerLine: 48, min: 2, max: 8 });
  const card = Boolean(transcriptFeedback || failed || closeConfirmOpen || downloadProgress || receipt);
  const tier = configOpen ? "panel" : card ? "card" : "pill";

  return (
    <HudSurface
      className="luna-voice"
      aria-label={t("title")}
      autoGrow
      tier={tier}
      onDismiss={requestClose}
    >
      <HudHeader
        icon={<MicIcon size={15} />}
        lead={lead}
        title={t("title")}
        // 联网指示在胶囊档同样常驻（ADR-111 ⑤⑴）：恰恰是胶囊在按住说话，音频正在离开设备。
        uplink={uplink ? { ...uplink, active: sessionActive } : null}
        lane={lane}
        actions={
          <>
            {/* 不挂 ⌘K：语音面板刻意永不成为键窗口（注入目标是源应用的光标），那一下组合键
                到的是用户正在用的应用，挂了也只是一句兑现不了的提示。 */}
            {/* 固定位置（PRD-02-019）：实心 / 描边两枚字形，不只靠按下态的底色。 */}
            <IconButton
              density="hud"
              label={pinned ? t("action.unpin") : t("action.pin")}
              icon={pinned ? <PinFilledIcon size={14} /> : <PinIcon size={14} />}
              aria-pressed={pinned}
              onClick={() => void actions.togglePinned()}
            />
            {/* 语音设置（面板档）：会话中不可改，故只在空闲时给。 */}
            {!sessionActive && !transcriptFeedback ? (
              <IconButton
                density="hud"
                label={t("action.settings")}
                icon={<SettingsIcon size={14} />}
                aria-pressed={configOpen}
                onClick={() => setConfigOpen((open) => !open)}
              />
            ) : null}
            {menuItems.length > 0 ? (
              <HudActionMenu label={t("action.more")} side="bottom" items={menuItems} />
            ) : null}
            <IconButton
              density="hud"
              label={t("action.close")}
              icon={<CloseIcon size={14} />}
              onClick={requestClose}
            />
          </>
        }
      />

      {closeConfirmOpen ? (
        <section className="luna-voice__close-confirm" role="alertdialog" aria-label={t("close.title")}>
          <strong>{t("close.title")}</strong>
          <span>{sessionActive ? t("close.activeBody") : t("close.resultBody")}</span>
          <div className="luna-voice__close-actions">
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-btn--sm"
              onClick={() => setCloseConfirmOpen(false)}
            >
              {t("close.keep")}
            </button>
            <button
              type="button"
              className="luna-btn luna-btn--danger luna-btn--sm"
              onClick={() => void discardAndClose()}
            >
              {t("close.discard")}
            </button>
          </div>
        </section>
      ) : null}

      {transcriptFeedback && !configOpen ? (
        /* 结果先编辑再落地（PRD-02-022）。⌘↵ = 主动作（↵ 留给换行）。 */
        <section className="luna-voice__result">
          <textarea
            className="luna-voice__result-edit"
            value={editedText}
            aria-label={t("preview.edit_label")}
            rows={resultRows}
            data-no-drag
            onChange={(event) => setEditedText(event.currentTarget.value)}
            onFocus={resultEditing.onFocus}
            onPointerDown={resultEditing.onPointerDown}
            onBlur={resultEditing.onBlur}
            onKeyDown={(event) => {
              // ⌘/Ctrl 组合键不经输入法上屏，无需组字判据。
              if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
                event.preventDefault();
                landPrimary();
              }
            }}
          />
          {/* 原文对照收进折叠项：多数时候用户只关心最终稿，原文是核对时才翻的。 */}
          {transcriptFeedback.refined && transcriptFeedback.raw !== transcriptFeedback.finalText && (
            <details className="luna-voice__source-preview" data-no-drag>
              <summary>{t("preview.show_raw")}</summary>
              <p>{transcriptFeedback.raw}</p>
            </details>
          )}
          <div className="luna-voice__result-actions">
            <button
              type="button"
              className="luna-btn luna-btn--primary luna-voice__land"
              aria-keyshortcuts="Meta+Enter Control+Enter"
              onClick={landPrimary}
            >
              {savedNotePath ? (
                <StickyNoteIcon size={14} />
              ) : primaryLanding ? (
                landingIcon(primaryLanding.mode, 14)
              ) : null}
              <span>
                {savedNotePath
                  ? t("landing.open_note")
                  : primaryLanding
                    ? t(primaryLanding.labelKey)
                    : null}
              </span>
              <kbd className="luna-voice__kbd">{formatShortcut("cmd_or_ctrl+enter")}</kbd>
            </button>
          </div>
        </section>
      ) : null}

      {downloadProgress}
      <HudReceipt receipt={receipt} onExpire={clearLandingReceipt} />

      {configOpen ? (
        <div className="luna-voice__control-panel" data-no-drag>
          <p className="luna-voice__settings-summary">
            {t("configuration.summary", {
              style: modeLabel(activeMode.label),
              destination: t(`landing.${state.landing_mode}`),
              input: t(state.input_mode === "hold" ? "mode.hold" : "mode.toggle"),
              processing: state.translate_enabled
                ? t("action.translate_output")
                : state.auto_refine
                  ? t("action.auto_refine")
                  : t("mode.raw"),
            })}
          </p>
          <label className="luna-voice__control-group">
            <span className="luna-voice__control-label">{t("mode.label")}</span>
            <SelectBox
              className="luna-voice__mode-select"
              value={activeModeId}
              onChange={(event) => setActiveModeId(event.currentTarget.value as VoiceRefineMode)}
            >
              {allModes.map((mode) => (
                <option key={mode.id} value={mode.id}>
                  {modeLabel(mode.label)}
                </option>
              ))}
            </SelectBox>
          </label>
          <label className="luna-voice__control-group">
            <span className="luna-voice__control-label">{t("landing.default_label")}</span>
            {/* 形态由注册表声明（`SettingControl::Select{Dropdown}`，AGENTS §4）；选项值同源
                注册表（useSettingOptions），不在此手抄清单。 */}
            <SelectBox
              className="luna-voice__mode-select"
              value={state.landing_mode}
              onChange={(event) => void actions.setLandingMode(event.currentTarget.value as LandingMode)}
            >
              {landingOptions.map((value) => (
                <option key={value} value={value}>
                  {t(`landing.${value}`)}
                </option>
              ))}
            </SelectBox>
          </label>
          <section className="luna-voice__control-group" aria-label={t("mode.input_label")}>
            <span className="luna-voice__control-label">{t("mode.input_label")}</span>
            <div className="luna-voice__segmented">
              <button
                type="button"
                className="luna-voice__segment"
                aria-pressed={state.input_mode === "hold"}
                onClick={() => void actions.setInputMode("hold")}
              >
                {t("mode.hold")}
              </button>
              <button
                type="button"
                className="luna-voice__segment"
                aria-pressed={state.input_mode === "toggle"}
                onClick={() => void actions.setInputMode("toggle")}
              >
                {t("mode.toggle")}
              </button>
            </div>
          </section>
          <section className="luna-voice__control-group" aria-label={t("action.processing_options")}>
            <span className="luna-voice__control-label">{t("action.processing_options")}</span>
            <div className="luna-voice__option-row">
              <button
                type="button"
                className="luna-hud-toggle"
                aria-pressed={state.auto_refine}
                onClick={() => void actions.setAutoRefine(!state.auto_refine)}
              >
                {state.auto_refine ? <CheckSquareIcon size={14} /> : <SquareIcon size={14} />}
                <span>{t("action.auto_refine")}</span>
              </button>
              {/* 只在落点是「存为速记」时出现：它作用于落盘之后的那一篇，注入 / 剪贴板没有这一篇。 */}
              {state.landing_mode === "quick_note" && quickNoteRefine !== null && (
                <button
                  type="button"
                  className="luna-hud-toggle"
                  aria-pressed={quickNoteRefine}
                  onClick={() => void actions.setQuickNoteRefine(!quickNoteRefine)}
                >
                  {quickNoteRefine ? <CheckSquareIcon size={14} /> : <SquareIcon size={14} />}
                  <span>{t("action.quick_note_refine")}</span>
                </button>
              )}
              <button
                type="button"
                className="luna-hud-toggle"
                aria-pressed={state.translate_enabled}
                onClick={() => void actions.setTranslateEnabled(!state.translate_enabled)}
              >
                {state.translate_enabled ? <CheckSquareIcon size={14} /> : <SquareIcon size={14} />}
                <span>{t("action.translate_output")}</span>
              </button>
              {state.translate_enabled && (
                <label className="luna-voice__target-select">
                  <span>{t("translation.target")}</span>
                  <SelectBox
                    className="luna-voice__mode-select"
                    value={state.translate_target_lang}
                    onChange={(event) => void actions.setTranslateTargetLang(event.currentTarget.value)}
                  >
                    {translateTargets.map((lang) => (
                      <option key={lang} value={lang}>
                        {t(langLabelKey(lang), { defaultValue: lang })}
                      </option>
                    ))}
                  </SelectBox>
                </label>
              )}
            </div>
          </section>
        </div>
      ) : null}
    </HudSurface>
  );
}
