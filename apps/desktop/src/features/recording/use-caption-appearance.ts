/**
 * 字幕外观设置读取（PRD-04-040）：`caption.*` 设备级设置 → 字幕面板呈现参数。
 *
 * 字幕窗为**预热常驻窗**（启动即挂载、之后只 show/hide，不重挂载），故不能只在 mount 读一次；
 * 订阅 `settings://changed`（Rust 统一广播）**热更新**，用户在设置页调整即时反映到字幕面板。
 */

import { useCallback, useEffect, useState } from "react";
import { commands, events, type SettingSpecDto } from "../../ipc/bindings";

export type CaptionFontSize = "small" | "medium" | "large" | "xlarge";
export type CaptionTextColor = "white" | "yellow" | "cyan" | "green";

export interface CaptionAppearance {
  fontSize: CaptionFontSize;
  /** 字幕文字颜色档（覆盖视频/沉浸场景生效；对标播放器字幕）。 */
  textColor: CaptionTextColor;
  /** 同屏最多字幕行数（1–6，clamp）。 */
  maxLines: number;
  /** 沉浸模式**文字衬底**不透明度（0–100；0 = 纯描边无底）。 */
  textOpacity: number;
  /** 沉浸模式**框体背景**不透明度（0–100；0 = 全透明悬浮文字）。 */
  frameOpacity: number;
  /** 翻译开着时是否也显示原文（false = 只看译文，电影字幕式）。 */
  showSource: boolean;
  /** 双语时译文在上、原文在下且小一号（电影双语字幕的排法）。 */
  translationFirst: boolean;
}

const DEFAULT_APPEARANCE: CaptionAppearance = {
  fontSize: "large",
  textColor: "white",
  maxLines: 3,
  textOpacity: 55,
  frameOpacity: 60,
  showSource: true,
  translationFirst: false,
};

const FONT_SIZES: readonly CaptionFontSize[] = ["small", "medium", "large", "xlarge"];
const TEXT_COLORS: readonly CaptionTextColor[] = ["white", "yellow", "cyan", "green"];

function clamp(value: number, min: number, max: number, fallback: number): number {
  if (!Number.isFinite(value)) return fallback;
  return Math.min(max, Math.max(min, Math.round(value)));
}

/** 字幕浮层里能直接改的外观项（ADR-137 H6：显示方式在 HUD 上切，不必进设置页）。 */
export type CaptionAppearancePatch =
  | { key: "caption.font_size"; value: CaptionFontSize }
  | { key: "caption.text_color"; value: CaptionTextColor }
  | { key: "caption.text_opacity" | "caption.frame_opacity"; value: number }
  | { key: "caption.show_source" | "caption.translation_first"; value: boolean };

export interface CaptionAppearanceControls extends CaptionAppearance {
  /**
   * 改一项外观。写的是同一个设备级设置——设置页那一项与这里是同一个值，不另存一份；
   * 先就地改再落盘（拖滑杆时等一次往返才变是拖不动的），落盘后的广播会回读同一个值。
   */
  setAppearance: (patch: CaptionAppearancePatch) => void;
}

function patchAppearance(current: CaptionAppearance, patch: CaptionAppearancePatch): CaptionAppearance {
  switch (patch.key) {
    case "caption.font_size":
      return { ...current, fontSize: patch.value };
    case "caption.text_color":
      return { ...current, textColor: patch.value };
    case "caption.text_opacity":
      return { ...current, textOpacity: clamp(patch.value, 0, 100, current.textOpacity) };
    case "caption.frame_opacity":
      return { ...current, frameOpacity: clamp(patch.value, 0, 100, current.frameOpacity) };
    case "caption.show_source":
      return { ...current, showSource: patch.value };
    case "caption.translation_first":
      return { ...current, translationFirst: patch.value };
  }
}

/** HUD 与离线设置示例共用同一外观投影。 */
export function captionAppearanceFromSettings(specs: readonly SettingSpecDto[]): CaptionAppearance {
  const next: CaptionAppearance = { ...DEFAULT_APPEARANCE };
  for (const spec of specs) {
    const value = spec.current_value;
    if (spec.key === "caption.font_size" && value.kind === "str") {
      if ((FONT_SIZES as readonly string[]).includes(value.value)) next.fontSize = value.value as CaptionFontSize;
    } else if (spec.key === "caption.text_color" && value.kind === "str") {
      if ((TEXT_COLORS as readonly string[]).includes(value.value)) next.textColor = value.value as CaptionTextColor;
    } else if (spec.key === "caption.max_lines" && value.kind === "int") {
      next.maxLines = clamp(value.value, 1, 6, DEFAULT_APPEARANCE.maxLines);
    } else if (spec.key === "caption.text_opacity" && value.kind === "int") {
      next.textOpacity = clamp(value.value, 0, 100, DEFAULT_APPEARANCE.textOpacity);
    } else if (spec.key === "caption.frame_opacity" && value.kind === "int") {
      next.frameOpacity = clamp(value.value, 0, 100, DEFAULT_APPEARANCE.frameOpacity);
    } else if (spec.key === "caption.show_source" && value.kind === "bool") {
      next.showSource = value.value;
    } else if (spec.key === "caption.translation_first" && value.kind === "bool") {
      next.translationFirst = value.value;
    }
  }
  return next;
}

export function useCaptionAppearance(): CaptionAppearanceControls {
  const [appearance, setAppearance] = useState<CaptionAppearance>(DEFAULT_APPEARANCE);

  useEffect(() => {
    let cancelled = false;

    async function reload() {
      const result = await commands.listSettings();
      if (cancelled || result.status !== "ok") return;
      setAppearance(captionAppearanceFromSettings(result.data));
    }

    void reload();
    const unlisten = events.settingsChanged.listen((event) => {
      if (event.payload.key.startsWith("caption.")) void reload();
    });
    return () => {
      cancelled = true;
      void unlisten.then((fn) => fn());
    };
  }, []);

  const setAppearanceSetting = useCallback((patch: CaptionAppearancePatch) => {
    setAppearance((current) => patchAppearance(current, patch));
    const value =
      typeof patch.value === "boolean"
        ? ({ kind: "bool", value: patch.value } as const)
        : typeof patch.value === "number"
          ? ({ kind: "int", value: Math.round(patch.value) } as const)
          : ({ kind: "str", value: patch.value } as const);
    void commands.setSetting(patch.key, value);
  }, []);

  return { ...appearance, setAppearance: setAppearanceSetting };
}
