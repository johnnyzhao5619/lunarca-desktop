// 回收站的排序口径（纯函数，PRD-06-009）。Headless 可测。
//
// **默认必须是"最近删除的在最上面"**：`.trash/.index.json` 是按删除顺序追加的，直接
// 列出来就是最早删的排第一——与访达、Notion、Drive 全部相反，而用户来回收站找的几乎
// 总是刚刚误删的那一条。后端已按删除时间倒序下发，这里保证**搜索之后顺序不变**：
// `FilteredList` 缺省按模糊匹配得分重排（命令面板要的就是相关度），而回收站是时间流，
// 输入两个字就把"刚刚那一条"甩到列表中间，是把它最主要的用途弄丢了。

import type { TrashEntryDto } from "../../ipc/bindings";
import { fileNameOf } from "../../lib/note-path";

/** 排序口径。默认 `deleted`（最近删除在前）。 */
export type TrashSortMode = "deleted" | "name" | "size";

/** 可选口径的呈现顺序（菜单按它渲染，不在组件里再写一份）。 */
export const TRASH_SORT_MODES: readonly TrashSortMode[] = ["deleted", "name", "size"];

/**
 * 某一口径下的比较器。
 *
 * 名称走自然序（`numeric` + `sensitivity: "base"`，与文件树 `sortTreeChildren` 同一口径
 * ——同一个库里的两处列表不该对"第 2 章 / 第 10 章"给出不同答案）；时间与占用都是**倒序**
 * ，因为这两问的都是"最新/最大的那几个是谁"。
 *
 * 每一档都以稳定次级键收尾（时间 → id，其余 → 删除时间），否则同值项的相对次序会随
 * 每次重渲染抖动。
 */
export function trashComparator(mode: TrashSortMode): (a: TrashEntryDto, b: TrashEntryDto) => number {
  return (a, b) => {
    if (mode === "name") {
      const byName = fileNameOf(a.original_path).localeCompare(fileNameOf(b.original_path), undefined, {
        numeric: true,
        sensitivity: "base",
      });
      if (byName !== 0) {
        return byName;
      }
    }
    if (mode === "size") {
      // 体积是 u64 字符串（本仓 DTO 约定）——用 BigInt 比，不经 Number 丢精度。
      const delta = BigInt(b.byte_size) - BigInt(a.byte_size);
      if (delta !== 0n) {
        return delta > 0n ? 1 : -1;
      }
    }
    const byTime = Number(b.deleted_ms) - Number(a.deleted_ms);
    return byTime !== 0 ? byTime : b.id.localeCompare(a.id);
  };
}
