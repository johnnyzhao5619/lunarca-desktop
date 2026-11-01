/**
 * 笔记正文里带时间戳的行（PRD-04-066）。
 *
 * 服务于「在笔记里就地听」：编辑面的时间戳行可点即跳、当前句随播放高亮。
 *
 * **表由 Rust 给**（`playback_timecode_lines`）：前端既不解析 `[HH:MM:SS.mmm]` 的格式，
 * 也不自己数行——行号口径必须与编辑器所见逐行对齐，而那份内容正是 Rust 读给编辑器的
 * 同一份（ADR-003）。
 *
 * **与回放视图的 cue 表刻意不是同一张**：那份是重切分后的可读 cue（与导出字幕同源），
 * 行数与正文行数不必相等。混成一张表的话，任一侧断行规则一变，另一侧就静默错位。
 */

import { useEffect, useState } from "react";
import { commands, events } from "../../ipc/bindings";

/**
 * 一条可听的正文行。
 *
 * 刻意不直接用绑定里的 `TimecodeLineDto`：specta 把 `f64` 映射成 `number | null`（NaN 的
 * 序列化语义），而这里的毫秒**永远是有值的**——空值在装载时就过滤掉，不让 `null` 一路
 * 渗进二分查找与行号计算里。
 */
export interface TimecodeLine {
  /** 1-based 行号，与编辑器所见一致。 */
  line: number;
  /** 会话绝对毫秒。 */
  startMs: number;
}

/**
 * 当前播放位置落在第几条时间戳行上；`-1` = 还没到第一条。
 *
 * 判据是「最后一条起点 ≤ 当前位置」——正文行没有结束时间，下一行的起点就是它的边界。
 * 表按起点升序（Rust 按行序输出，行序即时间序），故二分即可。
 */
export function activeTimecodeIndex(lines: readonly TimecodeLine[], positionMs: number): number {
  let low = 0;
  let high = lines.length - 1;
  let found = -1;
  while (low <= high) {
    const mid = (low + high) >> 1;
    const entry = lines[mid];
    if (entry !== undefined && entry.startMs <= positionMs) {
      found = mid;
      low = mid + 1;
    } else {
      high = mid - 1;
    }
  }
  return found;
}

/**
 * 载入某一篇的时间戳行表。
 *
 * @param notePath 笔记相对路径；`null` 或 `enabled=false` 时表为空、不发 IPC。
 * @param enabled 只有确定这一篇是录音/转写时才问——普通笔记问一次是白问一次往返。
 */
export function useTimecodeLines(notePath: string | null, enabled: boolean): TimecodeLine[] {
  const [lines, setLines] = useState<TimecodeLine[]>([]);

  useEffect(() => {
    if (!notePath || !enabled) {
      setLines([]);
      return;
    }
    let disposed = false;
    const load = async () => {
      const result = await commands.playbackTimecodeLines(notePath);
      if (disposed) return;
      // 读不到就当没有可听的行：就地起播降级为不出现，不弹错——它是增益，不是主线。
      setLines(
        result.status === "ok"
          ? result.data.flatMap((row) =>
              row.start_ms === null ? [] : [{ line: row.line, startMs: row.start_ms }],
            )
          : [],
      );
    };
    void load();

    // 二次转写 / 翻译落盘会整体改写正文，行号随之全变。
    const unlisten = events.vaultChanged.listen((event) => {
      if (event.payload.rel_path === notePath && !event.payload.deleted) void load();
    });
    return () => {
      disposed = true;
      void unlisten.then((dispose) => dispose());
    };
  }, [notePath, enabled]);

  return lines;
}
