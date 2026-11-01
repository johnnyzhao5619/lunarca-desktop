/**
 * 去抖后的值：输入停下来 `delayMs` 之后才跟上。
 *
 * 用在"这个值一变就要去后端取一次数"的地方——查询框每敲一个字都重取一次清单是白烧
 * 一轮 IPC，而中文输入法组字期间更是每个字母都会触发一次。
 *
 * 它去抖的是「值」而不是「那次调用」**：调用侧的去抖（`setTimeout` 包住 `commands.x`）
 * 挡不住把原值当依赖的 `useEffect` / 分页 key ——那些地方要的是一个慢一拍的值，
 * 而不是一次慢一拍的调用。速记侧栏的清单 key 就是这一类：key 一变整份清单重来，
 * 拿未去抖的值做 key 等于每敲一个字重置一次分页与勾选集。
 */
import { useEffect, useState } from "react";

export function useDebouncedValue<T>(value: T, delayMs: number): T {
  const [settled, setSettled] = useState(value);
  useEffect(() => {
    // 值回到与已生效的一致时（清空、撤回一个字又敲回去）不必再等一轮——重开的定时器
    // 会把这一帧的渲染推迟到没有意义的将来。
    if (Object.is(value, settled)) {
      return;
    }
    const timer = window.setTimeout(() => setSettled(value), delayMs);
    return () => window.clearTimeout(timer);
  }, [value, settled, delayMs]);
  return settled;
}
