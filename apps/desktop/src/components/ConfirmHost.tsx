// 破坏性动作确认宿主——全应用**唯一**的「你确定吗」。
//
// **此前同一件事有三种语言，外加两处压根不问**：
// - 原生 `window.confirm`（笔记/文件夹删除、清空回收站与永久删除、清空通知、回放
//   cue 删除、恢复本组设置——共 7 处）；
// - 面板内两步确认（版本回滚、知识度量清除、回放 cue 删除的**另一条**入口——同一个
//   动作从右键进是原生弹窗、从行内进是两步确认）；
// - `.luna-dialog` 确认卡（知识度量清除）。
// 而 **AI 会话删除与清空全部用量记录直接执行，一句都不问**——「动态」页里两个页签，
// 清空通知问一句、清空用量不问。
//
// 更要紧的是：`VersionHistoryPanel` 的注释里**早就写着** `window.confirm`「夺焦、
// 无法主题化」并据此改掉了自己那一处——而全应用最高频的删除路径用的恰是它。一条只
// 写在注释里的裁决，管不住下一个人。
//
// 形态与行为全部复用既有原语（`.luna-overlay` + `.luna-dialog` + `useDialogBehavior`），
// 因此确认卡天然带 Esc / 焦点圈定 / 焦点归还——那正是原生 confirm 给不了的东西。
//
// API 刻意做成**命令式 Promise**（`await confirm({...})`）而不是声明式组件：调用点
// 全在菜单回调与异步函数里，若要每处自己持一份 `open`/`pending` 状态，等于把状态
// 管理复制 9 遍。与本仓库既有的 `useContextMenu().open(...)` 同一条路子。

import { createContext, useCallback, useContext, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { useDialogBehavior } from "../hooks/use-dialog-behavior";

export interface ConfirmOptions {
  /** 标题：说清**要对什么做什么**，而不是「确认」。 */
  title: string;
  /** 补充说明：后果、影响范围、能不能撤销。可省。 */
  description?: string;
  content?: ReactNode;
  /** 确认按钮文案；缺省为通用「确认」。破坏性动作应给动词（「删除」「清空」）。 */
  confirmLabel?: string;
  /** 取消按钮文案；缺省为通用「取消」。 */
  cancelLabel?: string;
  /**
   * 是否为破坏性动作（默认 **true**）。
   *
   * 默认破坏性是刻意的：这个宿主存在的理由就是那些不可逆的操作，而"忘了标"应该退化成
   * 更醒目而不是更不醒目。
   */
  destructive?: boolean;
}

export type ConfirmFn = (options: ConfirmOptions) => Promise<boolean>;

const ConfirmContext = createContext<ConfirmFn | null>(null);

/** 取确认函数。宿主在 `main.tsx` 包住整棵树，故任意窗口的任意组件都可用。 */
export function useConfirm(): ConfirmFn {
  const confirm = useContext(ConfirmContext);
  if (!confirm) {
    throw new Error("useConfirm 必须在 <ConfirmHost> 内使用");
  }
  return confirm;
}

interface PendingConfirm extends ConfirmOptions {
  resolve: (value: boolean) => void;
}

export function ConfirmHost({ children }: { children: ReactNode }) {
  const [pending, setPending] = useState<PendingConfirm | null>(null);
  // 同一时刻只可能有一张确认卡；后来者若挤掉前一张，前一张的 Promise 必须以「取消」
  // 落地——否则调用方会永远等下去（而它多半正 `await` 在一条删除路径上）。
  const pendingRef = useRef<PendingConfirm | null>(null);
  pendingRef.current = pending;

  const settle = useCallback((value: boolean) => {
    const current = pendingRef.current;
    pendingRef.current = null;
    setPending(null);
    current?.resolve(value);
  }, []);

  const confirm = useCallback<ConfirmFn>(
    (options) =>
      new Promise<boolean>((resolve) => {
        pendingRef.current?.resolve(false);
        const next = { ...options, resolve };
        pendingRef.current = next;
        setPending(next);
      }),
    [],
  );

  return (
    <ConfirmContext.Provider value={confirm}>
      {children}
      {pending
        ? createPortal(<ConfirmDialog pending={pending} onSettle={settle} />, document.body)
        : null}
    </ConfirmContext.Provider>
  );
}

function ConfirmDialog({
  pending,
  onSettle,
}: {
  pending: PendingConfirm;
  onSettle: (value: boolean) => void;
}) {
  const { t } = useTranslation("common");
  const cancelRef = useRef<HTMLButtonElement>(null);
  const cancel = useCallback(() => onSettle(false), [onSettle]);
  // 初始焦点落在**取消**上：确认卡是一道闸，回车不该顺手把它推过去。
  const dialogRef = useDialogBehavior<HTMLDivElement>({
    onClose: cancel,
    initialFocusRef: cancelRef,
  });
  const destructive = pending.destructive ?? true;

  return (
    <div className="luna-overlay" role="presentation" onClick={cancel}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-confirm"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="luna-confirm-title"
        aria-describedby={pending.description ? "luna-confirm-desc" : undefined}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-dialog__header">
          <h3 id="luna-confirm-title">{pending.title}</h3>
          {pending.description ? <p id="luna-confirm-desc">{pending.description}</p> : null}
        </header>
        {pending.content}
        <div className="luna-dialog__actions">
          <button
            ref={cancelRef}
            type="button"
            className="luna-btn luna-btn--secondary"
            onClick={cancel}
          >
            {pending.cancelLabel ?? t("action.cancel")}
          </button>
          <button
            type="button"
            className={`luna-btn ${destructive ? "luna-btn--danger" : "luna-btn--primary"}`}
            onClick={() => onSettle(true)}
          >
            {pending.confirmLabel ?? t("action.confirm")}
          </button>
        </div>
      </div>
    </div>
  );
}
