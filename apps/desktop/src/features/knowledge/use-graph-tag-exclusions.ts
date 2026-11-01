// AI 关系标签排除清单（PRD-06-035c）：读取 + 增删，规则本体在 Rust
// （`Vault::graph_tag_exclusions`，随库存储于 `.luna/graph_scope.json`）。
//
// 多个消费面（笔记文件树 / 四条功能清单）各持一份本 Hook 实例；改动经
// `set_graph_tag_exclusion` 落库后 Rust 广播 `vault://changed`（rel_path =
// 排除清单文件），各实例据此重取——不引入前端侧的第二份共享状态。

import { useCallback, useEffect, useState } from "react";
import { commands, events } from "../../ipc/bindings";

/** 排除清单的库内落点（与 Rust `set_graph_tag_exclusion` 的广播路径同一契约）。 */
const GRAPH_SCOPE_META_PATH = ".luna/graph_scope.json";

export interface GraphTagExclusions {
  /** 当前排除文件夹清单（Vault 相对路径，Rust 侧已排序去重）。 */
  exclusions: readonly string[];
  /** 设定某文件夹的排除态；失败返回错误消息（成功返回 `null`）。 */
  setExcluded: (folder: string, excluded: boolean) => Promise<string | null>;
}

export function useGraphTagExclusions(): GraphTagExclusions {
  const [exclusions, setExclusions] = useState<readonly string[]>([]);

  useEffect(() => {
    let alive = true;
    const load = () => {
      void commands.graphTagExclusions().then((result) => {
        // Vault 未打开等错误按空清单处理：没有库就没有规则，不值得报错打扰。
        if (alive) {
          setExclusions(result.status === "ok" ? result.data : []);
        }
      });
    };
    load();
    const unlisten = events.vaultChanged.listen((event) => {
      if (event.payload.rel_path === GRAPH_SCOPE_META_PATH) {
        load();
      }
    });
    return () => {
      alive = false;
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  const setExcluded = useCallback(async (folder: string, excluded: boolean) => {
    const result = await commands.setGraphTagExclusion(folder, excluded);
    if (result.status === "error") {
      return result.error;
    }
    // 命令直接返回最新清单：本实例即刻生效，其余实例随广播跟上。
    setExclusions(result.data);
    return null;
  }, []);

  return { exclusions, setExcluded };
}
