// AI 结果落地后的确认条（PRD-06-021）：「已替换选区 · 撤销」。
//
// 落地此前是面板直接消失，用户不确定发生了什么、也找不到反悔的入口（撤销栈在，但没人
// 告诉他）。这条非模态横条停在编辑器底部几秒，撤销一键回到落地前；正文里同时有渐隐
// 高亮标出改了哪里（`ai-session.ts`）。

import { useEffect } from "react";
import { useTranslation } from "react-i18next";

/** 自动收起时长（毫秒）。 */
const AUTO_DISMISS_MS = 6_000;

interface AiLandedToastProps {
  message: string;
  onUndo: () => void;
  onDismiss: () => void;
}

export function AiLandedToast({ message, onUndo, onDismiss }: AiLandedToastProps) {
  const { t } = useTranslation("editor");

  useEffect(() => {
    const timer = window.setTimeout(onDismiss, AUTO_DISMISS_MS);
    return () => {
      window.clearTimeout(timer);
    };
  }, [onDismiss]);

  return (
    <div className="luna-ai-toast" role="status">
      <span>{message}</span>
      <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={onUndo}>
        {t("floating.landed.undo")}
      </button>
    </div>
  );
}
