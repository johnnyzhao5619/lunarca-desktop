import { useCallback, useEffect, useState } from "react";
import { commands, events } from "../../ipc/bindings";
import { useSettings } from "../settings/use-settings";
import { vaultErrorText } from "../../lib/vault-error";

/**
 * 译境会话开关的**单一取数口径**：运行态来自 Rust 的 `DesktopLensSession`，经
 * `desktop-lens://changed` 广播回流，故仪表盘卡片、顶栏常驻入口与全局快捷键三处永远一致
 * ——任一入口开关，另两处立刻同步，不各自维护一份 boolean（同 `useNotificationUnread` 先例）。
 */
export function useDesktopLens() {
  const { specs } = useSettings();
  const [active, setActive] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    void commands.desktopLensState().then((running) => {
      if (!disposed) setActive(running);
    });
    const unlisten = events.desktopLensChanged.listen((event) => {
      setActive(event.payload.active);
    });
    return () => {
      disposed = true;
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  const toggle = useCallback(async () => {
    setError(null);
    const result = await commands.desktopLensToggle();
    if (result.status === "error") {
      // 权限缺失等可执行失败由 Rust 给出文案；此处只如实回显，不再本地拼错误。
      setError(vaultErrorText(result.error));
      return;
    }
    setActive(result.data);
  }, []);

  const enabled = specs.find((spec) => spec.key === "labs.translate_space_enabled")?.current_value;
  return { active, enabled: enabled?.kind === "bool" && enabled.value, error, toggle };
}
