/**
 * 蒸馏动作的**派发口**：谁想触发，就从这里发一条事件（PRD-06-044 / PRD-06-045 / PRD-06-047）。
 *
 * **只派发、不自持状态、也不自己发命令**：确认卡、进行中进度与结果条由唯一常驻宿主
 * 承载（`useDistillBatch` / `useDistillPreview`），故任意目的地都能触发，不必先切到
 * 某一页。**结果的呈现也归宿主**——"被挡下的那几篇"要指名道姓 + 给理由，那一处实现
 * 已经存在；派发方自己发命令、把结果压成状态栏一行，用户就只读到「已加入 2 件；
 * 1 件未加入」（2026-09-02 真机）。
 *
 * 入口在对象所在处——用户想整理的是**眼前这个东西**，让他先切到某个设置页再选回来是把
 * 动作和对象拆开。三个触发面各自的对象不同：单篇（笔记页头/右键）、某个文件夹（四条
 * 清单与笔记树共用的 `folderMenuExtras` 挂点）、一批勾选的待整理内容（「速记」）。
 *
 * **文件名不再叫 `distill-folder-menu`**：这里四个导出里只有一个是文件夹菜单项，
 * 而「按真实 paths 汇总」压根与文件夹无关（ADR-087 拆掉了"目录决定动作"这条线）。
 * 名字与内容对不上时，下一个人要么找不到它、要么在别处再写一份。
 */

import type { TFunction } from "i18next";
import { SparklesIcon } from "../../components/ui/Icon";
import type { ContextMenuEntry } from "../../lib/context-menu";
import {
  DISTILL_BATCH_EVENT,
  DISTILL_NOTES_EVENT,
  DISTILL_PREVIEW_EVENT,
  MERGE_NOTES_EVENT,
  type DistillBatchRequestDetail,
  type DistillNotesDetail,
  type DistillPreviewRequestDetail,
  type MergeNotesDetail,
} from "../../lib/app-events";

/**
 * 请求对一篇资料做「重生成差异预览」（PRD-06-045）。
 *
 * `template`/`fullText` 省略时由 Rust 解析（已记录 > 建议）；显式给出即「换模板重新整理」
 * ——对编辑过的笔记，换模板同样只经预览这一条路，不给任何一键覆盖的入口。
 */
export function requestDistillPreview(
  notePath: string,
  options?: { template?: string; fullText?: boolean },
): void {
  window.dispatchEvent(
    new CustomEvent<DistillPreviewRequestDetail>(DISTILL_PREVIEW_EVENT, {
      detail: { notePath, template: options?.template, fullText: options?.fullText },
    }),
  );
}

/** 请求整理某个范围（`null` = 整个库）。 */
export function requestDistillBatch(scope: string | null): void {
  window.dispatchEvent(
    new CustomEvent<DistillBatchRequestDetail>(DISTILL_BATCH_EVENT, { detail: { scope } }),
  );
}

/**
 * 「把这几篇合成一篇知识笔记」（PRD-06-047）。
 *
 * 与 [`requestDistillNotes`] 刻意分开：那一条是"逐篇各出一篇"，这一条是"一堆合成一篇"，
 * 产出形态根本不同，共用一个标签会让用户预期错位。原笔记留在原地不删——整理是新增一份
 * 提炼，不是隐式销毁。
 *
 * **参数是真实 paths**：候选由用户当场选出（速记勾选集 / 文件树多选），不由某个物理
 * 目录回答"哪些算碎片"；对象资格由 Rust 判定并逐条下发（`can_merge`）。
 */
export function requestMergeNotes(paths: readonly string[]): void {
  window.dispatchEvent(
    new CustomEvent<MergeNotesDetail>(MERGE_NOTES_EVENT, { detail: { paths: [...paths] } }),
  );
}

/**
 * 「把这几篇整理成知识笔记」（PRD-06-044 / PRD-06-054）：**单篇与多选同一条路**。
 *
 * **只派发，不自己发命令**：命令的返回是"进了几件、哪些没进、为什么"，而"被挡下的
 * 那几篇怎么呈现"已经有唯一一处实现（批量结果卡，逐篇指名道姓 + 理由 + 可关）。这里
 * 曾经自己发命令并把结果压成状态栏一行，代价是用户只读到「已加入 2 件；1 件未加入」
 * ——是哪一篇、下一步怎么办，都没有地方回答（2026-09-02 真机）。
 */
export function requestDistillNotes(paths: readonly string[]): void {
  window.dispatchEvent(
    new CustomEvent<DistillNotesDetail>(DISTILL_NOTES_EVENT, { detail: { paths: [...paths] } }),
  );
}

export function distillFolderAction(folder: string, t: TFunction): ContextMenuEntry {
  // 根目录（空路径）不给——"整理整个库"是全库入口的事，混在某个文件夹的右键里
  // 会让人以为只整理这一层。
  if (folder === "") {
    return null;
  }
  return {
    kind: "action",
    id: "distillFolder",
    label: t("distill.batchFolderAction"),
    icon: <SparklesIcon size={13} />,
    onSelect: () => requestDistillBatch(folder),
  };
}
