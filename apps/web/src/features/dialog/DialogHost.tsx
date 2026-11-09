/**
 * 网页的确认卡与单行输入卡（与桌面 `ConfirmHost` / `PromptHost` 同一种交互）：命令式 Promise
 * （`await confirm({...})`），同一时刻只有一张，后来者挤掉前一张时前一张以「取消」落地。不用
 * `window.confirm`——它夺焦、不能主题化、文案由浏览器决定。
 *
 * 用原生 `<dialog>` 的模态：Esc 关闭、焦点圈在卡内、关掉后焦点回到打开它的那颗按钮，都由浏览器给；
 * 初始焦点：确认卡落在「取消」上（回车不该顺手把闸推过去），输入卡落在输入框上并全选。点到卡外的遮罩
 * 等于取消。输入法上屏那一下的回车不提交。
 */

import { createContext, useCallback, useContext, useEffect, useRef, useState, type FormEvent, type KeyboardEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { isImeCompositionKey } from "@lunarca/editor-core/ime-composition";

export interface ConfirmOptions {
  /** 说清要对什么做什么，而不是「确认」。 */
  title: string;
  /** 后果、影响范围、能不能撤销。 */
  description?: string;
  /** 确认按钮的动词（「删除」「退出」）；缺省为通用「确定」。 */
  confirmLabel?: string;
  /** 破坏性动作（默认是）：确认按钮按危险样式画。 */
  destructive?: boolean;
}

export interface PromptOptions {
  title: string;
  description?: string;
  /** 输入框（或下拉框）的可访问名。 */
  label: string;
  /** 初始值：打开即全选，直接打字就是覆盖。 */
  value: string;
  confirmLabel?: string;
  /** 给了就是下拉框（「移到…」选文件夹），值是各项的 `value`。 */
  choices?: ReadonlyArray<{ value: string; label: string }>;
}

export interface Dialogs {
  confirm(options: ConfirmOptions): Promise<boolean>;
  /** 取消返回 `null`；确认返回去掉首尾空白的输入（空输入等同取消）。下拉框原样返回所选的值。 */
  prompt(options: PromptOptions): Promise<string | null>;
}

const DialogContext = createContext<Dialogs | null>(null);

export function useDialogs(): Dialogs {
  const dialogs = useContext(DialogContext);
  if (!dialogs) throw new Error("useDialogs 必须在 <DialogHost> 内使用");
  return dialogs;
}

type Pending =
  | { kind: "confirm"; options: ConfirmOptions; resolve(value: boolean): void }
  | { kind: "prompt"; options: PromptOptions; resolve(value: string | null): void };

function cancelOf(pending: Pending): void {
  if (pending.kind === "confirm") pending.resolve(false);
  else pending.resolve(null);
}

export function DialogHost({ children }: { children: ReactNode }) {
  const [pending, setPending] = useState<Pending | null>(null);
  const pendingRef = useRef<Pending | null>(null);

  const show = useCallback((next: Pending) => {
    if (pendingRef.current) cancelOf(pendingRef.current);
    pendingRef.current = next;
    setPending(next);
  }, []);

  const settle = useCallback((run: (current: Pending) => void) => {
    const current = pendingRef.current;
    pendingRef.current = null;
    setPending(null);
    if (current) run(current);
  }, []);

  const [dialogs] = useState<Dialogs>(() => ({
    confirm: (options) => new Promise<boolean>((resolve) => show({ kind: "confirm", options, resolve })),
    prompt: (options) => new Promise<string | null>((resolve) => show({ kind: "prompt", options, resolve })),
  }));

  return (
    <DialogContext.Provider value={dialogs}>
      {children}
      {pending ? <DialogCard key={pending.kind + pending.options.title} pending={pending} settle={settle} /> : null}
    </DialogContext.Provider>
  );
}

function DialogCard({ pending, settle }: { pending: Pending; settle(run: (current: Pending) => void): void }) {
  const { t } = useTranslation("web");
  const ref = useRef<HTMLDialogElement>(null);
  const [value, setValue] = useState(pending.kind === "prompt" ? pending.options.value : "");
  const options = pending.options;
  const destructive = pending.kind === "confirm" && (pending.options.destructive ?? true);

  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    if (typeof dialog.showModal === "function") dialog.showModal();
    else dialog.setAttribute("open", "");
    const input = dialog.querySelector<HTMLInputElement>("input[type=text]");
    input?.select();
    return () => {
      if (dialog.open && typeof dialog.close === "function") dialog.close();
    };
  }, []);

  const cancel = () => settle(cancelOf);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    settle((current) => {
      if (current.kind === "confirm") current.resolve(true);
      else if (current.options.choices) current.resolve(value);
      else current.resolve(value.trim() ? value.trim() : null);
    });
  };
  const guardIme = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter" && isImeCompositionKey(event.nativeEvent)) event.preventDefault();
  };

  return (
    <dialog
      ref={ref}
      className="luna-web-dialog"
      role={pending.kind === "confirm" ? "alertdialog" : "dialog"}
      aria-labelledby="web-dialog-title"
      aria-describedby={options.description ? "web-dialog-desc" : undefined}
      onCancel={(event) => {
        event.preventDefault();
        cancel();
      }}
      onClick={(event) => {
        if (event.target === event.currentTarget) cancel();
      }}
    >
      <form className="luna-web-dialog__card" onSubmit={submit}>
        <h2 id="web-dialog-title" className="luna-web-dialog__title">
          {options.title}
        </h2>
        {options.description ? (
          <p id="web-dialog-desc" className="luna-web-hint">
            {options.description}
          </p>
        ) : null}
        {pending.kind === "prompt" ? (
          <label className="luna-web-field">
            <span className="luna-web-field__label">{pending.options.label}</span>
            {pending.options.choices ? (
              <select className="luna-web-input" value={value} autoFocus onChange={(event) => setValue(event.target.value)}>
                {pending.options.choices.map((choice) => (
                  <option key={choice.value} value={choice.value}>
                    {choice.label}
                  </option>
                ))}
              </select>
            ) : (
              <input
                className="luna-web-input"
                type="text"
                value={value}
                autoFocus
                onChange={(event) => setValue(event.target.value)}
                onKeyDown={guardIme}
              />
            )}
          </label>
        ) : null}
        <div className="luna-web-dialog__actions">
          <button type="button" className="luna-web-button" autoFocus={pending.kind === "confirm"} onClick={cancel}>
            {t("common.cancel")}
          </button>
          <button type="submit" className={`luna-web-button ${destructive ? "luna-web-button--danger" : "luna-web-button--primary"}`}>
            {options.confirmLabel ?? t("common.confirm")}
          </button>
        </div>
      </form>
    </dialog>
  );
}
