// 当前生效的删除策略（`vault.delete_mode`）——回收站页与全应用十二处删除确认卡的单一取数口。
//
// **一处取数，处处消费，不各问各的**：策略变了之后，如果各消费面各自缓存，会出现
// 「设置里已改成永久删除、确认卡还在说可以还原」这种最要命的分叉。故这里只有一个 hook，
// 且它订阅 Rust 广播的 `settings://changed`（`set_setting`/`reset_setting`/导入都经它回来，
// 跨窗口一致）与 `vault://switched`——策略是 **Vault 级**设置（`SettingScope::Vault`），
// 换库即换值。

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, events } from "../ipc/bindings";
import { deleteConsequenceKey, type DeleteModeState } from "../lib/delete-mode";

/** `vault.delete_mode` 的 registry key——订阅时按它筛，避免每条设置变更都重取。 */
const DELETE_MODE_KEY = "vault.delete_mode";

/**
 * 当前删除策略；未取回前为 `null`（调用方据此**不承诺任何可恢复性**，见 `delete-mode.ts`）。
 */
export function useDeleteMode(): DeleteModeState {
  const [mode, setMode] = useState<DeleteModeState>(null);

  const refresh = useCallback(() => {
    void commands.vaultDeleteMode().then(setMode);
  }, []);

  useEffect(() => {
    refresh();
    const unlistenSettings = events.settingsChanged.listen((event) => {
      if (event.payload.key === DELETE_MODE_KEY) {
        refresh();
      }
    });
    const unlistenVault = events.vaultSwitched.listen(() => {
      refresh();
    });
    return () => {
      for (const unlisten of [unlistenSettings, unlistenVault]) {
        void unlisten.then((dispose) => {
          dispose();
        });
      }
    };
  }, [refresh]);

  return mode;
}

/**
 * 删除确认卡里那句「删了之后会怎样」，已按当前策略取好文案。
 *
 * 十二处确认卡此前逐处硬写「可从回收站还原」——那句话在默认策略（系统废纸篓）下指错了
 * 地方，在 `permanent` 策略下是**反的**。调用点现在只问一句话，措辞由 [deleteConsequenceKey]
 * 单点决定。
 */
export function useDeleteConsequence(): string {
  const { t } = useTranslation("workspace");
  return t(deleteConsequenceKey(useDeleteMode()));
}
