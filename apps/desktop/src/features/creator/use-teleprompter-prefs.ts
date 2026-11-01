/**
 * 提词器的显示偏好（ADR-162 ⑤ C4a）：`teleprompter.*` 内部设置 → 面板呈现参数。
 *
 * 面板是常驻窗，只挂载一次，所以订阅 `settings://changed` 热更新。字号、行宽、速度的上下限与
 * 步长取自设置规格（Rust 注册表一处），这里不另写一份数。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { commands, events, type SettingSpecDto } from "../../ipc/bindings";

export type TeleprompterNumberKey = "teleprompter.font_size" | "teleprompter.line_width" | "teleprompter.speed";

export interface NumberPref {
  value: number;
  min: number;
  max: number;
  step: number;
}

export interface TeleprompterPrefs {
  fontSize: NumberPref;
  lineWidth: NumberPref;
  speed: NumberPref;
  mirror: boolean;
}

/** 规格还没取到时的占位（只为首帧不空；取到后一律以规格为准）。 */
const PLACEHOLDER: NumberPref = { value: 0, min: 0, max: 0, step: 0 };
const INITIAL: TeleprompterPrefs = {
  fontSize: PLACEHOLDER,
  lineWidth: PLACEHOLDER,
  speed: PLACEHOLDER,
  mirror: false,
};

function numberPref(spec: SettingSpecDto): NumberPref | null {
  if (spec.control.kind !== "number" || spec.current_value.kind !== "int") return null;
  return {
    value: spec.current_value.value,
    min: spec.control.min,
    max: spec.control.max,
    step: spec.control.step,
  };
}

export function prefsFromSettings(specs: readonly SettingSpecDto[]): TeleprompterPrefs {
  const next: TeleprompterPrefs = { ...INITIAL };
  for (const spec of specs) {
    if (spec.key === "teleprompter.mirror" && spec.current_value.kind === "bool") {
      next.mirror = spec.current_value.value;
      continue;
    }
    const pref = numberPref(spec);
    if (!pref) continue;
    if (spec.key === "teleprompter.font_size") next.fontSize = pref;
    else if (spec.key === "teleprompter.line_width") next.lineWidth = pref;
    else if (spec.key === "teleprompter.speed") next.speed = pref;
  }
  return next;
}

const FIELD: Record<TeleprompterNumberKey, keyof Omit<TeleprompterPrefs, "mirror">> = {
  "teleprompter.font_size": "fontSize",
  "teleprompter.line_width": "lineWidth",
  "teleprompter.speed": "speed",
};

export interface TeleprompterPrefControls extends TeleprompterPrefs {
  /** 按规格步长加（+1）或减（-1）一档，夹在上下限内。 */
  stepNumber: (key: TeleprompterNumberKey, direction: 1 | -1) => void;
  setMirror: (mirror: boolean) => void;
}

export function useTeleprompterPrefs(): TeleprompterPrefControls {
  const [prefs, setPrefs] = useState<TeleprompterPrefs>(INITIAL);
  /** 此刻的偏好（步进按它算；写设置不放进 setState 的更新函数里，那里要保持纯）。 */
  const prefsRef = useRef(prefs);
  prefsRef.current = prefs;

  useEffect(() => {
    let cancelled = false;
    async function reload() {
      const result = await commands.listSettings();
      if (cancelled || result.status !== "ok") return;
      setPrefs(prefsFromSettings(result.data));
    }
    void reload();
    const unlisten = events.settingsChanged.listen((event) => {
      if (event.payload.key.startsWith("teleprompter.")) void reload();
    });
    return () => {
      cancelled = true;
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  const stepNumber = useCallback((key: TeleprompterNumberKey, direction: 1 | -1) => {
    const field = FIELD[key];
    const pref = prefsRef.current[field];
    if (pref.step === 0) return;
    const value = Math.min(pref.max, Math.max(pref.min, pref.value + direction * pref.step));
    if (value === pref.value) return;
    setPrefs((current) => ({ ...current, [field]: { ...current[field], value } }));
    void commands.setSetting(key, { kind: "int", value });
  }, []);

  const setMirror = useCallback((mirror: boolean) => {
    setPrefs((current) => ({ ...current, mirror }));
    void commands.setSetting("teleprompter.mirror", { kind: "bool", value: mirror });
  }, []);

  return { ...prefs, stepNumber, setMirror };
}
