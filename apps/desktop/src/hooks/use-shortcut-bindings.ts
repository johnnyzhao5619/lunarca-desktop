/**
 * 一组快捷键设置的真值 + 双击时间窗（PRD-09-005；W3 起应用内快捷键全部可改键）。
 *
 * 工作区（命令键、按钮上的键位提示）与编辑器（`⌘E` / `⌘J` 这类整篇动作）都从这里取：默认值
 * 与真值只在 Rust 设置注册表，前端不写第二份；改键后随 `settings://changed` 即时生效。
 *
 * `keys` 必须是模块级常量——它进依赖数组，组件里每帧新建一个数组就等于每帧重取。
 */

import { useEffect, useState } from "react";
import { commands, events } from "../ipc/bindings";

/** 应用内双击判定与全局键共用同一个时间窗设置。 */
const DOUBLE_TAP_WINDOW_KEY = "shortcuts.double_tap_window_ms";

export interface ShortcutBindingState {
  /** `shortcuts.*` → 键值串（没绑键 = 空串；尚未读到 = 不在表里）。 */
  bindings: Readonly<Record<string, string>>;
  /** 双击时间窗（ms）；`null` = 尚未读到，暂不判双击。 */
  doubleTapWindowMs: number | null;
}

export function useShortcutBindings(keys: readonly string[]): ShortcutBindingState {
  const [state, setState] = useState<ShortcutBindingState>({ bindings: {}, doubleTapWindowMs: null });
  useEffect(() => {
    let disposed = false;
    const load = () => {
      void commands.listSettings().then((result) => {
        if (disposed || result.status !== "ok") return;
        const specs = result.data;
        const bindings = Object.fromEntries(
          keys.map((key) => {
            const value = specs.find((spec) => spec.key === key)?.current_value;
            return [key, value?.kind === "str" ? value.value : ""];
          }),
        );
        const window = specs.find((spec) => spec.key === DOUBLE_TAP_WINDOW_KEY)?.current_value;
        setState({
          bindings,
          doubleTapWindowMs: window?.kind === "int" && window.value > 0 ? window.value : null,
        });
      });
    };
    load();
    const unlisten = events.settingsChanged.listen((event) => {
      if (event.payload.key === DOUBLE_TAP_WINDOW_KEY || keys.includes(event.payload.key)) {
        load();
      }
    });
    return () => {
      disposed = true;
      void unlisten.then((dispose) => dispose());
    };
  }, [keys]);
  return state;
}
