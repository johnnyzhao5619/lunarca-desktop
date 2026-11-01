/**
 * 语言选项的**取用口径**（清单本身在 Rust 注册表：`luna_config::TRANSLATE_TARGET_LANGS` /
 * `TRANSCRIBE_LANG_HINTS`，随设置规格的 `control.options` 下发）。
 *
 * 为什么不在前端再存一份数组：设置写入现在会按注册表选项集合校验，前端一旦多列一种语言，
 * 用户选了就被后端拒绝——两份清单必然漂移成事故。故前端只保留「标签怎么取」这件呈现事。
 */

import { useEffect, useState } from "react";
import { commands, type SettingSpecDto } from "../../ipc/bindings";

/** 语言码 → i18n 标签 key（common 命名空间）。 */
export function langLabelKey(code: string): string {
  return `common:lang.${code}`;
}

/** 设置规格快照（选项集合是 Rust 编译期常量，一次取用即可，故进程内缓存一份 Promise）。 */
let specsPromise: Promise<SettingSpecDto[]> | null = null;

function loadSpecs(): Promise<SettingSpecDto[]> {
  specsPromise ??= commands
    .listSettings()
    .then((result) => (result.status === "ok" ? result.data : []))
    .catch(() => []);
  return specsPromise;
}

/**
 * 读取某个设置项的合法选项集合（非枚举型或尚未载入时为空数组）。
 *
 * 供设置页之外的表面（录音 HUD / 字幕面板 / 回放翻译目标）取同一份语言清单——
 * 它们写的正是这些设置键，选项自然应当同源。
 */
export function useSettingOptions(key: string): string[] {
  const [options, setOptions] = useState<string[]>([]);
  useEffect(() => {
    let alive = true;
    void loadSpecs().then((specs) => {
      if (!alive) return;
      const spec = specs.find((item) => item.key === key);
      setOptions(spec?.control.kind === "select" ? spec.control.options : []);
    });
    return () => {
      alive = false;
    };
  }, [key]);
  return options;
}

/** 转写语种提示所在的设置键（录音与实时字幕共用同一口径）。 */
export const TRANSCRIBE_LANG_SETTING = "realtime.transcribe_lang";

/** 转译目标语言所在的设置键（录音/字幕面板与回放翻译共用）。 */
export const TRANSLATE_TARGET_SETTING = "realtime.translate_target_lang";

/** 语音输入的翻译目标语言设置键（HUD 内切换的就是它）。 */
export const VOICE_INPUT_TARGET_SETTING = "voice_input.translate_target_lang";
