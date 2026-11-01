/**
 * 读一个设置的当前值，并随 `settings://changed` 更新（布尔与字符串各一个入口，读取同一处）。
 *
 * 判定与默认值都在 Rust 的设置注册表（ADR-130）；`fallback` 只是取回之前那一拍的显示值。
 */

import { useEffect, useState } from "react";

import { commands, events, type SettingValueDto } from "../ipc/bindings";

function useSettingValue<T>(key: string, fallback: T, pick: (value: SettingValueDto) => T | undefined): T {
  const [value, setValue] = useState(fallback);
  useEffect(() => {
    let disposed = false;
    const load = () => {
      void commands.listSettings().then((result) => {
        if (disposed || result.status !== "ok") return;
        const current = result.data.find((spec) => spec.key === key)?.current_value;
        const picked = current ? pick(current) : undefined;
        if (picked !== undefined) setValue(picked);
      });
    };
    load();
    const unlisten = events.settingsChanged.listen((event) => {
      if (event.payload.key === key) load();
    });
    return () => {
      disposed = true;
      void unlisten.then((dispose) => dispose());
    };
    // `pick` 是调用方的纯取值函数，按 key 重订阅即可。
  }, [key]);
  return value;
}

export function useBoolSetting(key: string, fallback: boolean): boolean {
  return useSettingValue(key, fallback, (current) => (current.kind === "bool" ? current.value : undefined));
}

export function useStringListSetting(key: string, fallback: readonly string[]): readonly string[] {
  return useSettingValue(key, fallback, (current) => (current.kind === "string_list" ? current.value : undefined));
}

export function useStringSetting(key: string, fallback: string): string {
  return useSettingValue(key, fallback, (current) => (current.kind === "str" ? current.value : undefined));
}
