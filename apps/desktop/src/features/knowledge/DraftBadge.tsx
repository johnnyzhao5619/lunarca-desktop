/**
 * 「记录」身份徽标（PRD-06-052/053 / ADR-086）——记录态笔记页头上的沉淀锚点。
 *
 * 与「资料」徽标（`MaterialBadge`）同一条纪律的第二个用户：**身份不能只活在行为结果里**
 * （问答降权、不自动打标、图谱默认隐藏），否则用户打开一篇草稿只会问"它跟我别的笔记有
 * 什么不一样"。答案就该在他正看着的那一篇上。
 *
 * 两者**刻意不同形**（虚线 vs 实线描边）：资料是"别人写的"，记录是"你写的但还没定稿"，
 * 待遇完全不同；两枚同形只差一个字，在页头那一排里等于没区分。
 *
 * 与资料徽标的另一处不同：这一枚**是可点的**——沉淀是一步动作（去掉 `luna.stage` 一个
 * 键），不像蒸馏那样要跑模型、要选模板、要看预览，故不值得为它开一层浮层或确认卡。
 * 但它**绝无自动档**（PRD-06-053）：把草稿判成知识只有用户有资格，这里就是那个
 * 「用户点了」的地方。
 *
 * **它旁边还有一枚「生成知识笔记」**（PRD-06-054 的 AI 辅助沉淀）——两个动作各占
 * 一枚徽标，刻意不合并成一枚带菜单的。理由写在 `DistillBadge` 的头注里（一处说清，
 * 这里不复写）：一句话是二者不互斥，AI 辅助沉淀**不改源记录的 stage**。
 *
 * 判定在 Rust（`note_draft_state` 下发已判定的布尔），前端不认识 `luna.stage` 的任何取值
 * （ADR-003 / `scripts/check-note-stage-single-source.py`）；取数、订阅与失效走
 * `useNoteDraftState`——与编辑器右键「标记为记录」同一份（PRD-06-058）。
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "../../ipc/bindings";
import { useNoteDraftState } from "./use-note-draft-state";

interface DraftBadgeProps {
  /** 笔记相对路径；`null` = 没有笔记可判定，不渲染。 */
  notePath: string | null;
  /** 沉淀完成后的落点（速记的沉淀会搬进 `notes_dir`，路径因此变）。 */
  onSettled?: (to: string) => void;
}

export function DraftBadge({ notePath, onSettled }: DraftBadgeProps) {
  const { t } = useTranslation("workspace");
  const { draft, refresh } = useNoteDraftState(notePath);
  const [busy, setBusy] = useState(false);

  if (!draft || !notePath) {
    return null;
  }
  return (
    <button
      type="button"
      className="luna-draft-badge luna-draft-badge--action"
      disabled={busy}
      title={t("draft.badgeHint")}
      onClick={() => {
        setBusy(true);
        void commands.settleNoteAsKnowledge(notePath).then((result) => {
          setBusy(false);
          if (result.status === "error") {
            return;
          }
          // 沉淀成功即刻重取：写盘事件也会带来同一个结论，但那是异步的，
          // 而用户刚点下去的那一枚徽标该当场消失。
          void refresh();
          if (result.data !== notePath) {
            onSettled?.(result.data);
          }
        });
      }}
    >
      {t("tree.draft")}
    </button>
  );
}
