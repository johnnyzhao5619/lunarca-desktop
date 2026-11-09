/**
 * 译文独立窗（PRD-04-044）：原文字幕窗的**被动镜像**，仅显示译文，可独立拖动/调整大小/定位。
 *
 * 与原文字幕窗**样式与交互对齐**（ADR-137 H6）：纯内容浮层 + 同一枚工具药丸（子集：背景浓淡、
 * 关闭），同一套外观设置（字号/颜色/不透明度，背景 0 = 沉浸）。不持有麦克风会话（原文窗单一
 * 持有），监听 `caption://view` 广播渲染译文行。关闭即退出「译文独立窗」模式。
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { IconButton } from "../../components/ui/IconButton";
import { HudSurface } from "../../components/ui/HudSurface";
import { HudHeader } from "../../components/ui/HudHeader";
import { cssVariables } from "../../lib/css-variables";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { commands } from "../../ipc/bindings";
import { LiveSubtitleView } from "../recording/LiveSubtitleView";
import { focusTranslatedView, type SubtitleView } from "../recording/live-subtitle";
import { CaptionsIcon, CloseIcon, SettingsIcon } from "@lunarca/editor-core/Icon";
import { CaptionAppearancePanel } from "./CaptionAppearancePanel";
import { useCaptionWindowHidden } from "./use-caption-window-hidden";
import { useCaptionAppearance } from "../recording/use-caption-appearance";
import { MIRROR_SETTING_KEYS } from "../recording/use-translation-mirror";
import { CAPTION_VIEW_EVENT, type CaptionViewPayload } from "../recording/caption-view-event";
import type { RecordingMode } from "../recording/use-recording";

const EMPTY_VIEW: SubtitleView = { cues: [], hypotheses: [] };

export function CaptionTranslationWindow() {
  const { t } = useTranslation("caption");
  const appearance = useCaptionAppearance();

  // 背景浓淡为 0 即沉浸（与原文窗同一个设置、同一条判据）。
  const immersive = appearance.frameOpacity === 0;
  const [appearanceOpen, setAppearanceOpen] = useState(false);
  // 这扇窗常被原文窗收起（选回同窗双语、会话结束）：收起时面板一并收起，下次打开不带着它。
  useCaptionWindowHidden("live-caption-translation", () => setAppearanceOpen(false));
  const [view, setView] = useState<SubtitleView>(EMPTY_VIEW);
  const [active, setActive] = useState(false);
  const ownerRef = useRef<{
    source: RecordingMode | null;
    sessionId: string | null;
    active: boolean;
  }>({ source: null, sessionId: null, active: false });
  // 沉浸焦点取**最新一条有译文的 cue**：说话时最新的是还没有译文的暂定句，按原文窗的焦点
  // 规则取它，这扇只显示译文的窗就整窗空白（ADR-137 H6 修复）。
  const displayedView = useMemo(
    () => (immersive ? focusTranslatedView(view) : view),
    [immersive, view],
  );

  useEffect(() => {
    const unlisten = listen<CaptionViewPayload>(CAPTION_VIEW_EVENT, (event) => {
      const owner = ownerRef.current;
      if (
        owner.active &&
        !event.payload.active &&
        (event.payload.source !== owner.source || event.payload.sessionId !== owner.sessionId)
      ) {
        return;
      }
      ownerRef.current = {
        source: event.payload.source,
        sessionId: event.payload.sessionId,
        active: event.payload.active,
      };
      setView(event.payload.view);
      setActive(event.payload.active);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  const closeSeparate = () => {
    const source = ownerRef.current.source;
    if (source !== null) {
      void commands.setSetting(MIRROR_SETTING_KEYS[source], { kind: "bool", value: false });
    }
    void commands.captionTranslationDismiss();
  };

  return (
    <HudSurface
      className="luna-caption luna-caption-look"
      aria-label={t("translationTitle")}
      data-immersive={immersive}
      data-session={active ? "open" : "idle"}
      data-panel-open={appearanceOpen || undefined}
      data-font-size={appearance.fontSize}
      data-text-color={appearance.textColor}
      style={cssVariables({
        "--caption-bg-alpha": String(appearance.textOpacity / 100),
        "--caption-frame-alpha": String(appearance.frameOpacity / 100),
      })}
      // Esc 先收外观面板，再关闭即退出「译文独立窗」模式（ADR-111 ⑤⑵）。
      onDismiss={() => {
        if (appearanceOpen) {
          setAppearanceOpen(false);
          return;
        }
        closeSeparate();
      }}
    >
      <LiveSubtitleView
        view={displayedView}
        showSource={false}
        showTranslation
        variant="caption"
        emptyLabel={active ? t("panel.waitingTranslation") : t("panel.idleTranslation")}
        sourceAriaLabel={t("translationTitle")}
      />

      {/* 与原文窗同一枚工具药丸（悬停 / 聚焦浮现）：身份沿用字幕家族的 Captions——它是原文窗
          的镜像，不是翻译弹窗；状态随主会话。 */}
      <HudHeader
        className="luna-caption__toolbar"
        data-no-drag
        icon={<CaptionsIcon size={15} />}
        title={t("translationTitle")}
        // 窗口身份写出来（真机 2026-10-02：透明背景下两扇字幕窗分不清哪扇是哪扇）。
        context={t("translationChip")}
        status={{
          kind: active ? "processing" : "idle",
          label: active ? t("status.listening") : t("status.idle"),
        }}
        actions={
          <>
            <IconButton
              density="hud"
              label={t("appearance.label")}
              icon={<SettingsIcon size={14} />}
              aria-expanded={appearanceOpen}
              onClick={() => setAppearanceOpen((open) => !open)}
            />
            <IconButton
              density="hud"
              label={t("action.closeTranslation")}
              icon={<CloseIcon size={14} />}
              onClick={closeSeparate}
            />
          </>
        }
      />
      {appearanceOpen ? <CaptionAppearancePanel appearance={appearance} /> : null}
    </HudSurface>
  );
}
