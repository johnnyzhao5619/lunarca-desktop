/**
 * 速记主区顶部的「整理」栏（PRD-06-058 C）：选中一条速记时，它的四条出路并排可见。
 *
 * **与行内 hover 动作同源**：转为正式笔记 / AI 整理成知识笔记 / 点开细写 / 删除——
 * 这里只是把侧栏那一行 hover 才露出的动作，在主区上以常驻形态再给一次（主区宽、看得清，
 * 且用户正对着这一篇的正文做决定）。判定与落盘全在 Rust，本组件只派发。
 *
 * 前两项必须明确区别（PRD-06-058 C）：转为正式笔记改变**这一篇**的类型与成熟度；AI 整理
 * 另建一篇带来源的知识笔记，源速记留在原地。二者不得合并成一个含糊的"生成知识"按钮。
 */

import { useTranslation } from "react-i18next";
import { BookOpenIcon, PencilIcon, SparklesIcon, TrashIcon } from "../../components/ui/Icon";

export interface QuickNoteActionsProps {
  onSettle: () => void;
  onDistill: () => void;
  onOpenInNotes: () => void;
  onDelete: () => void;
}

export function QuickNoteActions({
  onSettle,
  onDistill,
  onOpenInNotes,
  onDelete,
}: QuickNoteActionsProps) {
  const { t } = useTranslation(["workspace", "common"]);
  return (
    <div className="luna-quick-note-actions" role="toolbar" aria-label={t("draft.actionsLabel")}>
      <button type="button" className="luna-btn luna-btn--primary luna-btn--sm" onClick={onSettle}>
        <BookOpenIcon size={13} />
        <span>{t("draft.settle")}</span>
      </button>
      <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={onDistill}>
        <SparklesIcon size={13} />
        <span>{t("distill.generate")}</span>
      </button>
      <button
        type="button"
        className="luna-btn luna-btn--secondary luna-btn--sm"
        onClick={onOpenInNotes}
      >
        <PencilIcon size={13} />
        <span>{t("draft.openInNotes")}</span>
      </button>
      <span className="luna-quick-note-actions__spacer" aria-hidden="true" />
      {/* 删除按注册表策略走系统回收站 / 库内回收站 / 永久（`vault.delete_mode`），确认卡由调用方给。 */}
      <button
        type="button"
        className="luna-btn luna-btn--secondary luna-btn--sm luna-quick-note-actions__delete"
        onClick={onDelete}
      >
        <TrashIcon size={13} />
        <span>{t("common:action.delete")}</span>
      </button>
    </div>
  );
}
