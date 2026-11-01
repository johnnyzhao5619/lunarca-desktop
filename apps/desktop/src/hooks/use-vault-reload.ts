/**
 * 「库变了就重取一次」——**唯一一份**（PRD-06-059 / TDD-07 §4.2d）。
 *
 * `vault://changed` 的 `rel_path` 是给**按路径过滤**的消费者用的（回放、蒸馏徽标、词典
 * 面板只认自己那一条）；而整份清单、图谱、阅读视图这类消费者压根不看它——它们要的是
 * 「库变了，重取」。这个 hook 服务的正是后者。
 *
 * **合并同一阵变更**：一次批量移动/删除会逐条广播（每一条都是真实的事实），一次外部
 * 保存也可能连发几条。不合并的表现是一次拖拽引发 N 次全量重取，全部串在 Vault 锁上——
 * 2026-09-02 真机「拖完之后界面不动」有一半是这么放大出来的。
 *
 * **它的存在还消灭了两份手抄的去抖**：阅读视图与图谱此前各自写了一份 `setTimeout` +
 * 各自的 `RELOAD_DEBOUNCE_MS`，而对话历史、标签浏览、文件树与度量面板一份也没有——
 * 同一件事在六处有三种做法，正是"改了一处不会有任何东西变红"的形状。
 *
 * **索引扫描做完也算「库变了」**（V1b）：开库那一轮增量扫描期间，清单、知识库、体检读到的是
 * 半截索引；扫描逐批写索引、不发 `vault://changed`（那是逐文件的事实，几千篇各发一条只会
 * 刷屏），于是扫描结束后这些视图会一直停在半截。终态帧里确有篇数变了，就照常重取一次。
 */

import { useEffect, useRef } from "react";
import { events } from "../ipc/bindings";

/**
 * 合并窗口（ms）：一次批量的连发落在同一拍里，用户等不到它，也不会看见中间态。
 *
 * **一个档位，不给调用方调**：十二个消费者各自挑一个数，等于把"多久算同一阵"这件事
 * 又摊回各处——那正是收口之前的样子（三份手抄的 `setTimeout`，各带一个同名常量）。
 */
const RELOAD_DEBOUNCE_MS = 400;

export interface VaultReloadOptions {
  /** 为假时只是不重取——订阅照常挂着，故开关翻回来立刻生效，不必重挂监听。 */
  enabled?: boolean;
}

export function useVaultReload(reload: () => void, options: VaultReloadOptions = {}): void {
  const { enabled = true } = options;
  // 恒调最新那一份实现：重取函数几乎都是每次渲染新建的，跟着它重挂监听会在合并窗口里
  // 把已排上的那次取消掉（表现是"某些变更永远不刷新"）。
  const reloadRef = useRef(reload);
  reloadRef.current = reload;
  const enabledRef = useRef(enabled);
  enabledRef.current = enabled;

  useEffect(() => {
    let timer = 0;
    const schedule = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        if (enabledRef.current) {
          reloadRef.current();
        }
      }, RELOAD_DEBOUNCE_MS);
    };
    const unlistenChanges = events.vaultChanged.listen(schedule);
    const unlistenScan = events.scanProgress.listen((event) => {
      if (event.payload.done && event.payload.indexed > 0) {
        schedule();
      }
    });
    return () => {
      window.clearTimeout(timer);
      void unlistenChanges.then((dispose) => dispose());
      void unlistenScan.then((dispose) => dispose());
    };
  }, []);
}
