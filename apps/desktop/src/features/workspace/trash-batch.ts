// 批量还原的**结果口径**（纯函数，PRD-06-009）。Headless 可测。
//
// 一批还原有三种去向：还回来了、原位置被占（可以留副本）、真失败（权限 / 只读盘）。
// 此前它们各自往状态栏写一句，而状态栏**只有一行**——后写的把先写的盖掉，最后那句
// 「已还原 N 项」尤其致命：它会把"另外两条没回来"整个抹掉，用户看到的是一句纯好消息。
// 一次批量动作只该有**一句总结**，把三种去向一次说完。

/** 一批还原逐条落下来的去向。 */
export interface RestoreBatchTally {
  /** 真正还回来的条数（含用户确认「还原为副本」之后补回来的）。 */
  restored: number;
  /** 原位置被占、用户没有选择留副本的条数。 */
  conflicted: number;
  /** 真失败的条数（权限 / 只读盘 / 条目已不在）。 */
  failed: number;
}

/** 总结用的一条 i18n 消息。 */
export interface RestoreBatchMessage {
  /** `workspace` 命名空间下的 key。 */
  key: string;
  params: Record<string, number | string>;
}

/**
 * 把一批还原的去向总结成**一句话**；全 0（用户在冲突卡上点了取消且没有别的结果）时返回
 * `null`，此时不写状态栏——一次什么都没发生的操作不该留下一句话。
 *
 * 判据按"用户最需要知道的那件事"排：**没回来的比回来的重要**。全成功才只说成功；只要有
 * 没回来的，就必须在同一句里说清有几条、为什么。
 */
export function summarizeRestoreBatch(
  tally: RestoreBatchTally,
  /**
   * 头一条真失败的原因（已翻成人话）。
   *
   * **一条都没回来时必须把原因带上**：那种处境下用户手里没有任何别的线索，一句
   * 「3 项未能还原」等于让他去猜是磁盘只读还是权限不足。部分成功时不带——他可以对剩下
   * 那几条单独再试一次，那时会拿到逐条的精确原因。
   */
  reason?: string,
): RestoreBatchMessage | null {
  const { restored, conflicted, failed } = tally;
  const missed = conflicted + failed;
  if (missed === 0) {
    return restored > 0 ? { key: "main.restoredBatch", params: { count: restored } } : null;
  }
  if (restored === 0) {
    // 一条都没回来：直接说没成，别用"已还原 0 项"这种绕着说的话。
    if (conflicted > 0 && failed === 0) {
      return { key: "main.restoreBatchAllConflicted", params: { count: conflicted } };
    }
    return {
      key: "main.restoreBatchAllFailed",
      params: { count: missed, reason: reason ?? "" },
    };
  }
  return {
    key: "main.restoreBatchPartial",
    params: { restored, missed },
  };
}
