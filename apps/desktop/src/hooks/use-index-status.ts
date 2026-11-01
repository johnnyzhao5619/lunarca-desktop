/**
 * 笔记索引的扫描现状（PRD-06-004 ④，TDD-06 §4.1）。
 *
 * 开库后的增量扫描在后台跑，期间列表、搜索、快速切换器照常可用——但结果可能不全。消费者
 * 据此说「索引中（x / y），结果可能不全」，不拿空结果冒充「没有」。
 *
 * 首帧取 `vault_index_status`：启动装载的那一轮在窗口挂监听之前就开始了，只听事件会错过
 * 前几帧、把正在进行的扫描看成「没在扫」。之后由 `scan://progress` 推进。前端零业务
 * （ADR-003）：这里只做取数与订阅。
 */

import { useEffect, useState } from "react";

import { commands, events, type IndexStatusDto } from "../ipc/bindings";

/** 扫描中且确有要处理的篇数时返回现状，否则 `null`（没在扫 / 什么都没变）。 */
export function useIndexStatus(): IndexStatusDto | null {
  const [status, setStatus] = useState<IndexStatusDto | null>(null);

  useEffect(() => {
    let disposed = false;
    // 事件先到就以事件为准：查询回来得晚，不能用旧快照盖掉新进度。
    let heard = false;
    void commands.vaultIndexStatus().then((snapshot) => {
      if (!disposed && !heard) setStatus(snapshot);
    });
    const unlisten = events.scanProgress.listen((event) => {
      heard = true;
      const { indexed, total, done } = event.payload;
      setStatus({ scanning: !done, indexed, total });
    });
    return () => {
      disposed = true;
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  return status?.scanning && status.total > 0 ? status : null;
}
