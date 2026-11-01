// 装机就绪状态的订阅（PRD-14-063/064）。事实源是 Rust `setup_status`；这里只在它可能
// 变了的时候重读：相关设置键变了、模型下载完成、窗口重新获得焦点（用户去系统设置授权
// 后切回来）。「完成设置」横幅、启动概览门控、就地引导读的都是这一份。

import { createContext, useCallback, useContext, useEffect, useState } from "react";
import { commands, events, type SetupStatusDto } from "../../ipc/bindings";
import { ONBOARDING_CHANGED_EVENT } from "../../lib/app-events";
import { TRANSCRIPTION_CONSENT_KEY } from "../settings/TranscriptionConsentRow";

/** 会让三态变化的设置键（与 `use-onboarding` 同一份口径）。 */
export const SETUP_SETTING_KEYS: ReadonlySet<string> = new Set([
  "onboarding.completed",
  "onboarding.completed_steps",
  "onboarding.first_task_done",
  "onboarding.setup_reminder_dismissed",
  TRANSCRIPTION_CONSENT_KEY,
  "storage.vault_root",
]);

export interface SetupBannerProjection {
  /** 已完成的可补齐项数。 */
  done: number;
  /** 可补齐项总数。 */
  total: number;
  /** 尚未完成的步骤 id（按引导顺序）。 */
  remainingIds: string[];
  /** 第一个尚未完成的步骤（点横幅进入这里）；`null` = 没有要提示的。 */
  firstRemaining: string | null;
}

/**
 * 纯投影：横幅 / 概览那一行要显示什么。
 *
 * 「哪些步骤算数」由 Rust 的 `tracked_ids` 裁决，这里只按它给的 id 去 `steps` 里数三态。
 * 前端此前自带一份 `TRACKED_STEP_IDS = [vault, models, permissions, first_task]`，注释还写着
 * 「与 Rust 口径一致」——那是手工对齐，而口径一变（权限全被拒时不再计入）它就当场说谎，
 * 且两边谁也不会因此变红。
 *
 * 用户关掉提醒后一律返回空投影：横幅与「有事才弹」共读这一个事实，只关一半等于没关。
 */
export function projectSetupBanner(setup: SetupStatusDto): SetupBannerProjection {
  const empty = { done: 0, total: 0, remainingIds: [], firstRemaining: null };
  if (setup.reminder_dismissed) {
    return empty;
  }
  const stateOf = new Map(setup.steps.map((step) => [step.id, step.state]));
  const remaining = setup.tracked_ids.filter((id) => stateOf.get(id) !== "done");
  return {
    done: setup.tracked_ids.length - remaining.length,
    total: setup.tracked_ids.length,
    remainingIds: remaining,
    firstRemaining: remaining[0] ?? null,
  };
}

/** App 层唯一持有的订阅句柄；横幅、概览门控、引导向导都消费它，不各订各的。 */
export interface SetupStatusHandle {
  setup: SetupStatusDto | null;
  /** 重读并返回最新状态（未启用时返回 `null`）。 */
  refresh: () => Promise<SetupStatusDto | null>;
}

const SetupStatusContext = createContext<SetupStatusHandle>({
  setup: null,
  refresh: () => Promise.resolve(null),
});

export const SetupStatusProvider = SetupStatusContext.Provider;

export function useSetupStatusContext(): SetupStatusHandle {
  return useContext(SetupStatusContext);
}

export function useSetupStatus(enabled = true): SetupStatusHandle {
  const [setup, setSetup] = useState<SetupStatusDto | null>(null);

  const refresh = useCallback(async () => {
    if (!enabled) {
      return null;
    }
    const next = await commands.setupStatus();
    setSetup(next);
    return next;
  }, [enabled]);

  useEffect(() => {
    if (!enabled) {
      return;
    }
    void refresh();
    const onWindowEvent = () => {
      void refresh();
    };
    window.addEventListener(ONBOARDING_CHANGED_EVENT, onWindowEvent);
    window.addEventListener("focus", onWindowEvent);
    const unlistenSettings = events.settingsChanged.listen((event) => {
      if (SETUP_SETTING_KEYS.has(event.payload.key)) {
        void refresh();
      }
    });
    const unlistenDownload = events.modelDownloadProgress.listen((event) => {
      if (event.payload.done) {
        void refresh();
      }
    });
    const unlistenVault = events.vaultSwitched.listen(() => {
      void refresh();
    });
    // 登录 / 退出改变算力落点，「登录（可跳过）」那一步的状态跟着变。
    const unlistenAccount = events.accountChanged.listen(() => {
      void refresh();
    });
    // 引擎就绪核查完本机清单 → 「文本模型就绪」的答案可能变了（此前要等窗口重新聚焦）。
    const unlistenSetup = events.setupChanged.listen(() => {
      void refresh();
    });
    return () => {
      window.removeEventListener(ONBOARDING_CHANGED_EVENT, onWindowEvent);
      window.removeEventListener("focus", onWindowEvent);
      void unlistenSettings.then((dispose) => {
        dispose();
      });
      void unlistenDownload.then((dispose) => {
        dispose();
      });
      void unlistenVault.then((dispose) => {
        dispose();
      });
      void unlistenAccount.then((dispose) => {
        dispose();
      });
      void unlistenSetup.then((dispose) => {
        dispose();
      });
    };
  }, [enabled, refresh]);

  return { setup, refresh };
}
