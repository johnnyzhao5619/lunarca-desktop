/**
 * 提词器的偏好（ADR-162 ⑤ C4a、ADR-176 ④）：`teleprompter.*` 内部设置 → 面板呈现参数与跟读开关。
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
  /** 跟读总开关（ADR-176 ④）：用户开、默认关；关着时面板不显示跟读控件。 */
  followEnabled: boolean;
  /** 尽量不被录屏（ADR-176 ⑤）：默认开；只在「提词器」应用生效时起作用（判定在 Rust）。 */
  captureHidden: boolean;
  /** 开始前倒计时秒数（ADR-176 ⑤）：取值与选项来自注册表。 */
  countdownSecs: number;
  countdownOptions: readonly number[];
  /** 遥控模式（ADR-176 ⑥）：默认关。 */
  remoteEnabled: boolean;
}

/** 规格还没取到时的占位（只为首帧不空；取到后一律以规格为准）。 */
const PLACEHOLDER: NumberPref = { value: 0, min: 0, max: 0, step: 0 };
const INITIAL: TeleprompterPrefs = {
  fontSize: PLACEHOLDER,
  lineWidth: PLACEHOLDER,
  speed: PLACEHOLDER,
  mirror: false,
  followEnabled: false,
  captureHidden: true,
  countdownSecs: 0,
  countdownOptions: [],
  remoteEnabled: false,
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
    if (spec.key === "teleprompter.follow_enabled" && spec.current_value.kind === "bool") {
      next.followEnabled = spec.current_value.value;
      continue;
    }
    if (spec.key === "teleprompter.remote_enabled" && spec.current_value.kind === "bool") {
      next.remoteEnabled = spec.current_value.value;
      continue;
    }
    if (spec.key === "teleprompter.capture_hidden" && spec.current_value.kind === "bool") {
      next.captureHidden = spec.current_value.value;
      continue;
    }
    if (spec.key === "teleprompter.countdown_secs" && spec.current_value.kind === "str") {
      next.countdownSecs = Number(spec.current_value.value) || 0;
      if (spec.control.kind === "select") next.countdownOptions = spec.control.options.map(Number);
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

type NumberField = "fontSize" | "lineWidth" | "speed";

const FIELD: Record<TeleprompterNumberKey, NumberField> = {
  "teleprompter.font_size": "fontSize",
  "teleprompter.line_width": "lineWidth",
  "teleprompter.speed": "speed",
};

export interface TeleprompterPrefControls extends TeleprompterPrefs {
  /** 按规格步长加（+1）或减（-1）一档，夹在上下限内。 */
  stepNumber: (key: TeleprompterNumberKey, direction: 1 | -1) => void;
  setMirror: (mirror: boolean) => void;
  /** 写跟读总开关；返回写入是否成功（「开启跟读并开始」要等它写好再开）。 */
  setFollowEnabled: (enabled: boolean) => Promise<boolean>;
  setCaptureHidden: (hidden: boolean) => void;
  setCountdownSecs: (secs: number) => void;
  setRemoteEnabled: (enabled: boolean) => void;
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

  const setFollowEnabled = useCallback(async (enabled: boolean) => {
    setPrefs((current) => ({ ...current, followEnabled: enabled }));
    const reply = await commands.setSetting("teleprompter.follow_enabled", { kind: "bool", value: enabled });
    return reply.status === "ok";
  }, []);

  const setCaptureHidden = useCallback((hidden: boolean) => {
    setPrefs((current) => ({ ...current, captureHidden: hidden }));
    void commands.setSetting("teleprompter.capture_hidden", { kind: "bool", value: hidden });
  }, []);

  const setCountdownSecs = useCallback((secs: number) => {
    setPrefs((current) => ({ ...current, countdownSecs: secs }));
    void commands.setSetting("teleprompter.countdown_secs", { kind: "str", value: String(secs) });
  }, []);

  const setRemoteEnabled = useCallback((enabled: boolean) => {
    setPrefs((current) => ({ ...current, remoteEnabled: enabled }));
    void commands.setSetting("teleprompter.remote_enabled", { kind: "bool", value: enabled });
  }, []);

  return {
    ...prefs,
    stepNumber,
    setMirror,
    setFollowEnabled,
    setCaptureHidden,
    setCountdownSecs,
    setRemoteEnabled,
  };
}
