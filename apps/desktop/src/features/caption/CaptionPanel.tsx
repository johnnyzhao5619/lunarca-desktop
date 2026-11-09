/**
 * 实时字幕面板（PRD-04-040）：不录音、纯实时转写/转译字幕，用于跨语言面对面交流或实时翻译辅助。
 *
 * 对标 macOS 实时字幕 / Chrome Live Caption（ADR-137 H6）：**纯内容浮层**——没有标题行与
 * 页脚，控制收成一枚工具药丸，空闲时常显、会话中悬停或键盘聚焦才浮现；任何时候都能开始、
 * 能改语言。「框体 / 沉浸」两种模式合并为一根「背景浓淡」滑杆（0 = 沉浸：透明、只留最新
 * 一句）。复用录音同一引擎（`useRecording({ mode: "caption" })`，ADR-015 非激活 NSPanel）
 * ——业务全在 Rust，本组件只渲染。
 */

import { useMemo, useState } from "react";
import { IconButton } from "../../components/ui/IconButton";
import { HudSurface } from "../../components/ui/HudSurface";
import { HudHeader } from "../../components/ui/HudHeader";
import { cssVariables } from "../../lib/css-variables";
import { useTranslation } from "react-i18next";
import { useRecording, isRecordingSessionOpen } from "../recording/use-recording";
import { LiveSubtitleView } from "../recording/LiveSubtitleView";
import { SubtitleLangFlow } from "../recording/SubtitleLangFlow";
import {
  CaptionsIcon,
  CloseIcon,
  NotebookIcon,
  PlayIcon,
  SettingsIcon,
  StopIcon,
} from "@lunarca/editor-core/Icon";
import { HudReceipt, type HudReceiptValue } from "../../components/ui/HudReceipt";
import { CaptionAppearancePanel, type CaptionLayout } from "./CaptionAppearancePanel";
import { useCaptionWindowHidden } from "./use-caption-window-hidden";
import { pickRecordingBanner } from "../recording/recording-banner";
import { useCaptionAppearance } from "../recording/use-caption-appearance";
import { useTranslationMirror } from "../recording/use-translation-mirror";
import { focusSubtitleView, focusTranslatedView } from "../recording/live-subtitle";
import { goTo } from "../../lib/navigate";
import { vaultErrorText } from "../../lib/vault-error";

interface CaptionLanguageControlsProps {
  translateEnabled: boolean;
  transcribeLang: string;
  detectedLang: string | null;
  translateTarget: string;
  onTranscribeLangChange: (lang: string) => void;
  onTranslateTargetChange: (lang: string) => void;
  sourceAriaLabel: string;
  targetAriaLabel: string;
  dualSource: boolean;
  dualSourceLabel: string;
}

/**
 * 会话语言控制：源语言 → 目标语言（翻译开关并入「原文 / 双语 / 译文」显示方式）。
 * 组件只派发既有动作，设置持久化和活动会话热切仍由 useRecording 单点编排（ADR-003）。
 */
function CaptionLanguageControls({
  translateEnabled,
  transcribeLang,
  detectedLang,
  translateTarget,
  onTranscribeLangChange,
  onTranslateTargetChange,
  sourceAriaLabel,
  targetAriaLabel,
  dualSource,
  dualSourceLabel,
}: CaptionLanguageControlsProps) {
  return (
    // 双路各自检测的说明进悬停提示（真机 2026-10-02：它在工具药丸里占掉源语言下拉的宽，
    // 「自动检测」被挤成半截）。
    <div className="luna-caption__language-controls" title={dualSource ? dualSourceLabel : undefined}>
      <SubtitleLangFlow
        transcribeLang={dualSource ? "auto" : transcribeLang}
        detectedLang={detectedLang}
        onTranscribeLangChange={onTranscribeLangChange}
        transcribeDisabled={dualSource}
        translateActive={translateEnabled}
        translateTarget={translateTarget}
        onTranslateTargetChange={onTranslateTargetChange}
        sourceAriaLabel={sourceAriaLabel}
        targetAriaLabel={targetAriaLabel}
      />
    </div>
  );
}

type CaptionDisplay = "source" | "bilingual" | "translation";

/**
 * 显示方式三段（ADR-137 H6 补）：原文 = 不翻译；双语 = 原文 + 译文；译文 = 只看译文（看外语
 * 片的电影字幕式）。合并了此前的「翻译」开关——那是同一个选择的一半，分成两处就要学两次。
 * 两段都是持久化设置（`caption.translation_enabled` / `caption.show_source`），与设置页同值。
 */
function CaptionDisplaySwitch({
  value,
  onChange,
}: {
  value: CaptionDisplay;
  onChange: (next: CaptionDisplay) => void;
}) {
  const { t } = useTranslation("caption");
  return (
    <div className="luna-caption__segmented" role="group" aria-label={t("display.label")}>
      {(["source", "bilingual", "translation"] as const).map((mode) => (
        <button
          key={mode}
          type="button"
          className="luna-caption__segment"
          aria-pressed={value === mode}
          onClick={() => onChange(mode)}
        >
          {t(`display.${mode}`)}
        </button>
      ))}
    </div>
  );
}

export function CaptionPanel() {
  const { t } = useTranslation("caption");
  const { t: tCommon } = useTranslation("common");
  const { t: tRecording } = useTranslation("recording");
  const {
    state,
    transcript,
    hypotheses,
    translations,
    translateEnabled,
    translateTarget,
    transcribeLang,
    detectedLang,
    captureSource,
    executor,
    transcribeStopped,
    translateStopped,
    transcribeReconnecting,
    actions,
  } = useRecording({ mode: "caption" });

  const [saving, setSaving] = useState(false);
  const [appearanceOpen, setAppearanceOpen] = useState(false);
  // 收起时把外观面板一并收起：下次打开从字幕面开始，而不是上次没关的面板。
  useCaptionWindowHidden("live-caption", () => setAppearanceOpen(false));
  const [saveReceipt, setSaveReceipt] = useState<HudReceiptValue | null>(null);
  const appearance = useCaptionAppearance();
  // 背景浓淡为 0 即沉浸（ADR-137 H6）：透明、只留最新一句——不再有第二个「模式」开关。
  const immersive = appearance.frameOpacity === 0;

  async function onSaveNote() {
    setSaving(true);
    const result = await actions.saveNote();
    setSaving(false);
    setSaveReceipt(
      "path" in result
        ? {
            kind: "success",
            message: t("save.ok"),
            action: { label: t("save.open"), onSelect: () => goTo(`notes://${result.path}`) },
          }
        : { kind: "error", message: t("save.errorDetail", { detail: vaultErrorText(result.error) }) },
    );
  }

  const sessionOpen = isRecordingSessionOpen(state.status);

  const mirror = useTranslationMirror(
    "caption",
    transcript,
    hypotheses,
    translations,
    sessionOpen && translateEnabled,
    state.session_id,
  );
  const { separate, setSeparate, view } = mirror;
  const display: CaptionDisplay = !translateEnabled
    ? "source"
    : appearance.showSource
      ? "bilingual"
      : "translation";
  function changeDisplay(next: CaptionDisplay) {
    if (next === "source") {
      if (translateEnabled) actions.setTranslate(false);
      return;
    }
    if (!translateEnabled) actions.setTranslate(true);
    appearance.setAppearance({ key: "caption.show_source", value: next === "bilingual" });
    // 「只看译文」与「译文分窗」互斥：分窗时原文窗只放原文，再关掉原文这扇窗就空了。
    if (next === "translation" && separate) setSeparate(false);
  }
  // 原文窗此刻放什么：分窗时译文去了另一扇，这里只留原文；否则按显示方式。
  const showSourceHere = !translateEnabled || separate || appearance.showSource;
  const showTranslationHere = translateEnabled && !separate;
  const translationFirst =
    showSourceHere && showTranslationHere && appearance.translationFirst;
  // 双语排法只在「看双语」时有一个当前值；原文 / 只看译文时三格都不亮，点任一格即切到双语。
  const layout: CaptionLayout | null =
    display !== "bilingual" ? null : separate ? "separate" : translationFirst ? "translation_first" : "source_first";
  function changeLayout(next: CaptionLayout) {
    if (!translateEnabled) actions.setTranslate(true);
    if (!appearance.showSource) appearance.setAppearance({ key: "caption.show_source", value: true });
    if (next === "separate") {
      if (!separate) setSeparate(true);
      return;
    }
    if (separate) setSeparate(false);
    const first = next === "translation_first";
    if (appearance.translationFirst !== first) {
      appearance.setAppearance({ key: "caption.translation_first", value: first });
    }
  }
  // 沉浸（浓淡 0）只留最新一句；只看译文时取最新一条**已有译文**的句子——说话时最新的是还
  // 没有译文的暂定句，按原文焦点取它会整窗空白（与译文窗同一条修复）。
  const displayedView = useMemo(
    () => (immersive ? (showSourceHere ? focusSubtitleView(view) : focusTranslatedView(view)) : view),
    [immersive, showSourceHere, view],
  );
  const statusLabel =
    state.status === "recording"
      ? t("status.listening")
      : state.status === "stopping"
        ? t("status.stopping")
        : state.status === "paused"
          ? t("status.paused")
          : state.status === "error"
            ? t("status.error")
            : t("status.idle");

  // 同一时刻至多一条非阻断横幅（与录音 HUD 同一判据 `pickRecordingBanner`）：此前字幕窗把
  // 转写掉线 / 转译掉线 / 重连中各画一条，最多叠三条。
  const { shown: banner, folded: foldedBanners } = pickRecordingBanner({
    transcribeStopped,
    transcribeReconnecting,
    translateStopped,
    translateEnabled,
    degradedTier: null,
    degradedReason: null,
  });
  const bannerText = (key: string, fallbackKey: string) =>
    tRecording(key, { defaultValue: tRecording(fallbackKey) });

  // 唯一主动作：空闲 =「开始」（主色），会话中 =「停止」（中性强调，ADR-137 V3）。
  const lead = sessionOpen ? (
    <button
      type="button"
      className="luna-btn luna-btn--secondary luna-caption__primary"
      onClick={() => void actions.stop()}
      disabled={state.status === "stopping"}
    >
      <StopIcon size={13} />
      {state.status === "stopping" ? t("action.stopping") : t("action.stop")}
    </button>
  ) : (
    <button
      type="button"
      className="luna-btn luna-btn--primary luna-caption__primary"
      onClick={() => void actions.start()}
    >
      <PlayIcon size={13} />
      {t("action.start")}
    </button>
  );

  return (
    <HudSurface
      className="luna-caption luna-caption-look"
      aria-label={t("title")}
      data-immersive={immersive}
      data-session={sessionOpen ? "open" : "idle"}
      data-panel-open={appearanceOpen || undefined}
      data-order={translationFirst ? "translation-first" : undefined}
      data-font-size={appearance.fontSize}
      data-text-color={appearance.textColor}
      style={cssVariables({
        "--caption-bg-alpha": String(appearance.textOpacity / 100),
        "--caption-frame-alpha": String(appearance.frameOpacity / 100),
      })}
      // Esc 逃生通道（ADR-111 ⑤⑵）：先收外观面板（V8 退一层）；dismiss 经 stateRef 自守卫，
      // 会话进行中为 no-op——字幕窗本身就是产物，会话中的退出动作是「停止」。
      onDismiss={() => {
        if (appearanceOpen) {
          setAppearanceOpen(false);
          return;
        }
        void actions.dismiss();
      }}
    >
      <span className="luna-visually-hidden" role="status">
        {statusLabel}
      </span>

      {/* 非阻断横幅：字幕或译文停了 / 重连中——会话与音频不受影响。一次只出一条，其余进悬停说明。 */}
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
        </aside>
      )}

      {/* 字幕主体（独立窗模式下原文窗仅显示原文，译文交由独立译文窗） */}
      <LiveSubtitleView
        view={displayedView}
        showSource={showSourceHere}
        showTranslation={showTranslationHere}
        translationFirst={translationFirst}
        variant="caption"
        emptyLabel={sessionOpen ? t("panel.waiting") : t("panel.idle")}
        sourceAriaLabel={t("aria.subtitle")}
        trackLabels={{ self: tCommon("track.self"), remote: tCommon("track.remote") }}
      />

      {/* 工具药丸（ADR-137 H6 / ADR-112 ⑥⑵）：空闲常显；会话中悬停或键盘聚焦才浮现——任意
          聚焦即浮现，悬停才出现的东西必须有键盘等价物（V8）。联网指示是表面级插槽，就挂在
          这枚药丸上（任何背景浓淡下都在）。关闭仅非会话态（理由同 onDismiss）。 */}
      <HudHeader
        className="luna-caption__toolbar"
        data-no-drag
        icon={<CaptionsIcon size={15} />}
        lead={lead}
        title={t("title")}
        // 分窗时写明这扇是原文窗（真机 2026-10-02：透明背景下两扇字幕窗分不清）。
        context={translateEnabled && separate ? t("sourceChip") : undefined}
        uplink={executor}
        lane={
          <>
          <CaptionDisplaySwitch value={display} onChange={changeDisplay} />
          <CaptionLanguageControls
            translateEnabled={translateEnabled}
            transcribeLang={transcribeLang}
            detectedLang={detectedLang}
            translateTarget={translateTarget}
            onTranscribeLangChange={actions.setTranscribeLang}
            onTranslateTargetChange={actions.setTranslateTarget}
            sourceAriaLabel={t("option.source")}
            targetAriaLabel={t("option.target")}
            dualSource={captureSource === "both"}
            dualSourceLabel={t("source.dualAuto")}
          />
          </>
        }
        actions={
          <>
            {!sessionOpen && transcript.length > 0 ? (
              <IconButton
                density="hud"
                label={t("action.saveNote")}
                icon={<NotebookIcon size={14} />}
                disabled={saving}
                onClick={() => void onSaveNote()}
              />
            ) : null}
            {/* 外观与显示：字号、颜色、背景浓淡、逐行衬底、双语排法、分窗——都在这里就地改
                （真机 2026-10-02：「显示方式应该在 HUD 上就能切」）；不常用的在面板底部深链。 */}
            <IconButton
              density="hud"
              label={t("appearance.label")}
              icon={<SettingsIcon size={14} />}
              aria-expanded={appearanceOpen}
              onClick={() => setAppearanceOpen((open) => !open)}
            />
            {!sessionOpen ? (
              <IconButton
                density="hud"
                label={t("action.closePanel")}
                icon={<CloseIcon size={14} />}
                onClick={() => void actions.dismiss()}
              />
            ) : null}
          </>
        }
      />

      {appearanceOpen ? (
        <CaptionAppearancePanel
          appearance={appearance}
          session={{
            layout,
            onLayoutChange: changeLayout,
            captureSource,
            onCaptureSourceChange: actions.setCaptureSource,
            sessionOpen,
          }}
        />
      ) : null}

      <HudReceipt receipt={saveReceipt} onExpire={() => setSaveReceipt(null)} />
    </HudSurface>
  );
}
