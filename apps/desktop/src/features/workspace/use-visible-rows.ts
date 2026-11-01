/**
 * 把「此刻屏幕上有哪几行」报给多选（`use-note-selection.ts` 的 `visible`）。
 *
 * 看着只是一个 `useEffect`，但里面有两件必须每处都做对的事，而它此前被抄了三份
 * （`FolderedItems` / `FilteredNoteList` / `ChatHistoryList`）：
 *
 * ① **依赖是内容不是数组身份**：每次渲染都新建一个数组，按身份比会每一帧回调一次，
 *    而回调那头是 `setState`——那是一个不会停的渲染循环。
 * ② **回调本身不进依赖**：调用方大多传的是行内箭头函数，每帧都是新的；进了依赖同样每帧
 *    重跑。故用 ref 转一手。
 *
 * `paths` 传 `null` = 这一份不由我报（分组在场时只有 `FolderedItems` 知道折叠了哪些行，
 * 其余两个分支必须闭嘴，否则两个来源会互相盖）。
 */

import { useEffect, useRef } from "react";

export function useVisibleRowsReport(
  paths: readonly string[] | null,
  onVisibleRows?: (paths: string[]) => void,
): void {
  const onVisibleRowsRef = useRef(onVisibleRows);
  onVisibleRowsRef.current = onVisibleRows;
  // 内容指纹：`\n` 不会出现在 Vault 相对路径里。
  const key = paths === null ? null : paths.join("\n");
  useEffect(() => {
    if (key === null) {
      return;
    }
    onVisibleRowsRef.current?.(key === "" ? [] : key.split("\n"));
  }, [key]);
}
