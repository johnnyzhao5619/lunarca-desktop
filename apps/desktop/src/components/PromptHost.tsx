// 「给它起个名字」——全应用**唯一**的单行输入卡（PRD-06-006 / PRD-07-059）。
//
// 与 `ConfirmHost` 是同一条路子（命令式 Promise、同一套 `.luna-overlay` + `.luna-dialog`
// 原语、同一份 Esc / 焦点圈定 / 焦点归还），差别只在"确认卡问是与否，它问一个名字"。
//
// **它存在的理由是把重命名从某个视图的私产变回对象的能力**：改名此前只有文件树给得出
// ——因为实现方式被写死成"树里那一行变成输入框"。于是录音、转写、速记、搜索结果、图谱
// 节点右键里统统没有重命名，而它们展示的是同一批文件。行内输入是**树的**呈现优势（那一行
// 本来就只有一个名字），别处没有那一行，就该有一张卡：能力一份，形态按表面各取所长。

import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { useDialogBehavior } from "../hooks/use-dialog-behavior";
import { isImeKeyEvent } from "../lib/ime-key-event";

export interface PromptOptions {
  /** 标题：说清**在给什么起名字**（如「重命名「周会纪要」」）。 */
  title: string;
  /** 补充说明：会影响什么、不会影响什么。可省。 */
  description?: string;
  /** 输入框的可访问名与占位。 */
  label: string;
  /** 初始值（重命名 = 当前的名字，打开即全选，用户直接打字就是覆盖）。 */
  value: string;
  /** 确认按钮文案；缺省为通用「确认」。 */
  confirmLabel?: string;
}

/** 取消返回 `null`；确认返回**已 trim** 的输入（空输入等同取消）。 */
export type PromptFn = (options: PromptOptions) => Promise<string | null>;

const PromptContext = createContext<PromptFn | null>(null);

export function usePrompt(): PromptFn {
  const prompt = useContext(PromptContext);
  if (!prompt) {
    throw new Error("usePrompt 必须在 <PromptHost> 内使用");
  }
  return prompt;
}

interface PendingPrompt extends PromptOptions {
  resolve: (value: string | null) => void;
}

export function PromptHost({ children }: { children: ReactNode }) {
  const [pending, setPending] = useState<PendingPrompt | null>(null);
  // 同一时刻只可能有一张卡；后来者挤掉前一张时，前一张必须以「取消」落地——否则调用方
  // 会永远等下去（与 `ConfirmHost` 同一条纪律）。
  const pendingRef = useRef<PendingPrompt | null>(null);
  pendingRef.current = pending;

  const settle = useCallback((value: string | null) => {
    const current = pendingRef.current;
    pendingRef.current = null;
    setPending(null);
    current?.resolve(value);
  }, []);

  const prompt = useCallback<PromptFn>(
    (options) =>
      new Promise<string | null>((resolve) => {
        pendingRef.current?.resolve(null);
        const next = { ...options, resolve };
        pendingRef.current = next;
        setPending(next);
      }),
    [],
  );

  return (
    <PromptContext.Provider value={prompt}>
      {children}
      {pending
        ? createPortal(<PromptDialog pending={pending} onSettle={settle} />, document.body)
        : null}
    </PromptContext.Provider>
  );
}

function PromptDialog({
  pending,
  onSettle,
}: {
  pending: PendingPrompt;
  onSettle: (value: string | null) => void;
}) {
  const { t } = useTranslation("common");
  const [draft, setDraft] = useState(pending.value);
  const inputRef = useRef<HTMLInputElement>(null);
  const cancel = useCallback(() => onSettle(null), [onSettle]);
  const dialogRef = useDialogBehavior<HTMLDivElement>({
    onClose: cancel,
    initialFocusRef: inputRef,
  });

  // 全选**只在开卡这一次**：它的依赖必须是"这是哪一次提问"，而不是随每次按键都变的
  // 草稿。同一个错误在文件树的行内改名里真实发生过——输入一个字符就把整串重新选上，
  // 下一个字符直接把它覆盖掉，用户根本改不完一个名字（2026-09-02 真机上报）。
  useEffect(() => {
    inputRef.current?.select();
  }, []);

  const submit = () => {
    const value = draft.trim();
    onSettle(value === "" ? null : value);
  };

  return (
    <div className="luna-overlay" role="presentation" onClick={cancel}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-prompt"
        role="dialog"
        aria-modal="true"
        aria-labelledby="luna-prompt-title"
        aria-describedby={pending.description ? "luna-prompt-desc" : undefined}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-dialog__header">
          <h3 id="luna-prompt-title">{pending.title}</h3>
          {pending.description ? <p id="luna-prompt-desc">{pending.description}</p> : null}
        </header>
        <input
          ref={inputRef}
          type="text"
          className="luna-input"
          value={draft}
          aria-label={pending.label}
          placeholder={pending.label}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (isImeKeyEvent(event)) return;
            if (event.key === "Enter") {
              event.preventDefault();
              submit();
            }
          }}
        />
        <div className="luna-dialog__actions">
          <button type="button" className="luna-btn luna-btn--secondary" onClick={cancel}>
            {t("action.cancel")}
          </button>
          <button
            type="button"
            className="luna-btn luna-btn--primary"
            disabled={draft.trim() === ""}
            onClick={submit}
          >
            {pending.confirmLabel ?? t("action.confirm")}
          </button>
        </div>
      </div>
    </div>
  );
}
