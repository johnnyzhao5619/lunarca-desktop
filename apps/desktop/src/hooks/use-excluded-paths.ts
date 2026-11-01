// 排除路径（PRD-06-062，笔记库对标轮 V4a）：读取 + 增删。规则与全部过滤都在 Rust
// （`luna_vault::exclusions`，随库存储于 `.luna/excluded_paths.json`）；这里只取数与改。
// 文件树的淡显与右键勾选态**不用**这份清单——它们读 Rust 随树下发的 `excluded_by`
// （树随规则文件的广播重取），前端不自己拿规则比路径。
//
// 各消费面各持一份本 Hook 实例；改动经 `set_excluded_path` 落库后 Rust 广播 `vault://changed`
// （rel_path = 规则文件），各实例据此重取——不在前端另立一份共享状态。

import { useCallback, useEffect, useState } from "react";
import { commands, events } from "../ipc/bindings";

/** 规则文件的库内落点（与 Rust 广播路径同一契约）。 */
const EXCLUDED_PATHS_META_PATH = ".luna/excluded_paths.json";

export interface ExcludedPaths {
  /** 当前排除规则（库相对路径，Rust 侧已排序去重）。 */
  rules: readonly string[];
  /** 设定排除态；失败返回错误码串（成功返回 `null`）。 */
  setExcluded: (path: string, excluded: boolean) => Promise<string | null>;
}

export function useExcludedPaths(): ExcludedPaths {
  const [rules, setRules] = useState<readonly string[]>([]);

  useEffect(() => {
    let alive = true;
    const load = () => {
      void commands.excludedPaths().then((result) => {
        // 库没打开等错误按「没有排除」处理：排除是降噪，取不到就什么都不藏。
        if (alive) {
          setRules(result.status === "ok" ? result.data : []);
        }
      });
    };
    load();
    const unlistenChanged = events.vaultChanged.listen((event) => {
      if (event.payload.rel_path === EXCLUDED_PATHS_META_PATH) {
        load();
      }
    });
    const unlistenSwitched = events.vaultSwitched.listen(() => {
      load();
    });
    return () => {
      alive = false;
      for (const unlisten of [unlistenChanged, unlistenSwitched]) {
        void unlisten.then((dispose) => dispose());
      }
    };
  }, []);

  const setExcluded = useCallback(async (path: string, excluded: boolean) => {
    const result = await commands.setExcludedPath(path, excluded);
    if (result.status === "error") {
      return result.error;
    }
    setRules(result.data);
    return null;
  }, []);

  return { rules, setExcluded };
}
