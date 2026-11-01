/**
 * 库切换控制器（PRD-06-049 / ADR-025）。
 *
 * 护栏与代价**全部由 Rust 判定并下发**（`vault_switch_preflight`）——前端只负责：
 * 阻断时如实说、有代价时先问、切换中给过渡态。判据不在前端说第二遍（ADR-003）。
 */

import { useCallback, useState } from "react";
import { commands, type VaultSwitchPreflightDto } from "../../ipc/bindings";

export type VaultSwitchState =
  | { kind: "idle" }
  /** 预检中（毫秒级，仅防重复点击）。 */
  | { kind: "checking"; path: string; name: string }
  /** 录音进行中：阻断，不给"仍要切换"的出路——那是数据事故不是可接受的代价。 */
  | { kind: "blocked"; name: string }
  /** 有活动任务：说清代价后由用户确认。 */
  | { kind: "confirm"; path: string; name: string; preflight: VaultSwitchPreflightDto }
  /** 切换中：整个工作区待命，切换器转过渡态。 */
  | { kind: "switching"; name: string }
  | { kind: "error"; message: string };

export interface VaultSwitchController {
  state: VaultSwitchState;
  /** 发起切换（先预检）。 */
  request: (path: string, name: string) => void;
  /** 确认后真正切换。 */
  confirm: () => void;
  dismiss: () => void;
  /** 正在切换到的库名（切换器据此转过渡态）；`null` = 未在切换。 */
  switchingTo: string | null;
}

export function useVaultSwitch(options: { onSwitched: () => void }): VaultSwitchController {
  const [state, setState] = useState<VaultSwitchState>({ kind: "idle" });
  const { onSwitched } = options;

  const run = useCallback(
    async (path: string, name: string) => {
      setState({ kind: "switching", name });
      const result = await commands.switchVault(path);
      if (result.status === "ok") {
        setState({ kind: "idle" });
        onSwitched();
      } else {
        setState({ kind: "error", message: result.error });
      }
    },
    [onSwitched],
  );

  const request = useCallback(
    (path: string, name: string) => {
      setState({ kind: "checking", path, name });
      void commands.vaultSwitchPreflight(path).then((result) => {
        if (result.status === "error") {
          setState({ kind: "error", message: result.error });
          return;
        }
        const preflight = result.data;
        if (preflight.recording_active) {
          setState({ kind: "blocked", name });
          return;
        }
        // 目标失效或有活动任务 → 先把代价摆出来；两者都没有就直接切（不为零代价的操作
        // 多加一次点击）。
        if (preflight.pending_tasks > 0 || preflight.target_missing) {
          setState({ kind: "confirm", path, name, preflight });
          return;
        }
        void run(path, name);
      });
    },
    [run],
  );

  const confirm = useCallback(() => {
    setState((current) => {
      if (current.kind !== "confirm") {
        return current;
      }
      void run(current.path, current.name);
      return { kind: "switching", name: current.name };
    });
  }, [run]);

  const dismiss = useCallback(() => {
    // 切换在途不给关：此刻工作区正在换根，让用户以为"取消了"是谎报。
    setState((current) => (current.kind === "switching" ? current : { kind: "idle" }));
  }, []);

  return {
    state,
    request,
    confirm,
    dismiss,
    switchingTo: state.kind === "switching" ? state.name : null,
  };
}
