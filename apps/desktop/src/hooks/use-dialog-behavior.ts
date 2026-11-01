import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { isImeKeyEvent } from "../lib/ime-key-event";

/**
 * 模态对话框的**行为**（`.luna-overlay` + `.luna-dialog` 的另一半）——全应用唯一实现。
 *
 * `components/dialog.css` 收编的是对话框长什么样，这里收编的是它**怎么用**：
 *
 * **此前每张卡各写各的，于是能力参差**：模板选择器 / 图谱重扫 / 蒸馏批量 / 蒸馏预览 /
 * 回收站预览 / URL 导入 / 管理库 / 就地加词各自手写了一份 Esc 监听；知识体检 / 知识归入 /
 * 模板管理 / 切库**根本没有 Esc**——同为 `aria-modal="true"` 的模态，从图谱维护区唤起的
 * 三张卡里有两张关不掉。而**焦点圈定全仓只有一处有**（知识度量的清除确认），它还把
 * 「圈住哪些元素」写死成了两颗按钮。
 *
 * 三件事必须一起给，缺一件模态就不成立：
 * - **Esc 关闭**：键盘用户的逃生通道；
 * - **焦点圈定**：Tab 不得跑到卡片外面去——模态之外的东西此刻是不可交互的，焦点却能
 *   走过去，读屏用户会在一个"看不见的界面"里迷路；
 * - **焦点归还**：关掉后回到打开它的那颗按钮，而不是把焦点丢回 `<body>`。
 *
 * 焦点圈定**动态查询**卡内的可聚焦元素，不接受调用方传一份名单：名单会随内容变化过期
 * （对话框的按钮常随状态增减），而过期的名单意味着 Tab 会在某些状态下漏掉真实控件。
 */

/** 可聚焦元素选择器（排除 `tabindex="-1"` 与禁用项——它们不参与 Tab 序列）。 */
const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export interface DialogBehaviorOptions {
  /** Esc 与焦点归还的落点。对话框未挂载时不要调用本 hook（或传 `enabled: false`）。 */
  onClose: () => void;
  /**
   * 是否启用。用于「挂载但未打开」的对话框（如 `open` 受控的模板选择器）——
   * 关着的时候不该抢 Esc，否则同屏另一张卡按 Esc 会被它吃掉。
   */
  enabled?: boolean;
  /**
   * Esc 是否关闭。极少数卡在**有在途操作**时需要关掉这条（默认 true）。
   * 注意这与「点遮罩不关闭」是两件事：Esc 是明确意图，点遮罩多半是误触。
   */
  closeOnEscape?: boolean;
  /** 初始焦点落点；不给则落在卡内第一个可聚焦元素上。 */
  initialFocusRef?: RefObject<HTMLElement | null>;
  /**
   * 是不是**模态**（默认是）。
   *
   * 模态与非模态（Popover / 浮层菜单）在这两件事上**行为相反**，而 Esc 关闭、初始
   * 焦点、焦点归还三件事完全一样——所以是同一个 hook 加一个开关，不是两份实现：
   *
   * | | 模态 | 非模态 |
   * | --- | --- | --- |
   * | Tab 走到边界 | **圈住**：外面此刻不可交互，放它出去等于让读屏用户在看不见的界面里迷路 | **放行**：外面照常可用，圈住等于用户永远逃不出这个浮层 |
   * | 点到外面 | **不关**（多半是误触遮罩，且遮罩本来就挡住了点击） | **关闭**：浮层没有遮罩，点别处就是"我不要它了" |
   */
  modal?: boolean;
}

/**
 * @returns 挂到 `.luna-dialog` 根元素上的 ref（**callback ref**，`ref={...}` 用法不变）。
 *
 * **为什么是 callback ref 而不是 `useRef`**：初始焦点必须在卡片**真的挂载之后**落位，
 * 而 effect 的时机由依赖决定、与挂载无关。`AddContentPopover` 是最短的反例：它在
 * `useDialogBehavior` 之后写着 `if (!snapshot) return null`，于是首帧根本没有 ref'd 元素，
 * effect 照跑，`querySelectorAll` 查了个空，`initial?.focus()` 是空操作；快照到达后元素挂上了，
 * 但依赖没变、effect 不再跑，**焦点从此永远进不去**——用户打开浮层必须先按一次 Tab 才能用
 * 方向键（真机 2026-08-26 实测）。把元素挂载做成 state，"元素在场"就成了 effect 的依赖，
 * 时机由 React 保证，而不是靠每个调用方记得传对参数。
 */
export function useDialogBehavior<T extends HTMLElement = HTMLDivElement>({
  onClose,
  enabled = true,
  closeOnEscape = true,
  initialFocusRef,
  modal = true,
}: DialogBehaviorOptions): (node: T | null) => void {
  // 卡片根元素。放 state 而不是 ref：ref 赋值不会触发重渲染，effect 也就无从知道元素何时到场。
  const [dialog, setDialog] = useState<T | null>(null);
  // 身份必须稳定：每帧换一个新函数会让 React 每次都先以 `null` 卸载、再以元素装载一遍。
  const attach = useCallback((node: T | null) => setDialog(node), []);
  // 回调随每次渲染换新身份，若进依赖数组会让整个效果（含焦点归还）每帧重建。
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    if (!enabled || !dialog) {
      return;
    }
    const opener = document.activeElement as HTMLElement | null;

    // 初始焦点：调用方指定优先，否则卡内第一个可聚焦元素；都没有就聚焦卡片本身
    // （卡片带 `tabindex="-1"` 时生效，读屏据此把光标带进模态）。
    const focusables = () => Array.from(dialog.querySelectorAll<HTMLElement>(FOCUSABLE));
    const initial = initialFocusRef?.current ?? focusables()[0] ?? dialog;
    initial?.focus();

    const onKeyDown = (event: KeyboardEvent) => {
      // Esc 取消候选、回车上屏都是输入法的事，不是关对话框（PRD-07-040 ①）。
      if (isImeKeyEvent(event)) {
        return;
      }
      if (event.key === "Escape" && closeOnEscape) {
        event.stopPropagation();
        onCloseRef.current();
        return;
      }
      // 非模态：Tab 照常走出去（走出去即失去意义，见下面的 focusout 关闭）。
      if (event.key !== "Tab" || !modal) {
        return;
      }
      const items = focusables();
      if (items.length === 0) {
        // 卡内一个可聚焦元素都没有：Tab 只能停在卡片上，绝不放它出去。
        event.preventDefault();
        dialog.focus();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement;
      // 焦点已在卡外（上一次交互把它带走了）也要拽回来，而不只是在两端循环。
      if (!dialog.contains(active)) {
        event.preventDefault();
        (event.shiftKey ? last : first).focus();
        return;
      }
      if (event.shiftKey && active === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && active === last) {
        event.preventDefault();
        first.focus();
      }
    };

    // 非模态：点到浮层外面就是"我不要它了"。用 `pointerdown` 而不是 `click`——点在另一
    // 颗按钮上时，先关掉浮层再让那颗按钮响应，顺序反了会出现"点一次关不掉"（浮层的
    // 关闭发生在那颗按钮的动作之后，又被重新打开）。
    const onPointerDown = (event: Event) => {
      const target = event.target as Node | null;
      if (target && !dialog.contains(target)) {
        onCloseRef.current();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    if (!modal) {
      document.addEventListener("pointerdown", onPointerDown, true);
    }
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("pointerdown", onPointerDown, true);
      // 焦点归还：只在焦点仍留在卡内（或已掉回 body）时归还——用户若已主动点向别处，
      // 强行拽回去反而是打断。
      const active = document.activeElement;
      const stillInside = dialog.contains(active);
      if ((stillInside || active === document.body) && opener?.isConnected) {
        opener.focus();
      }
    };
    // 依赖只有这三个是刻意的：`initialFocusRef` 是 ref 容器（身份稳定），`onClose` 走
    // `onCloseRef` 转发——把它放进依赖会让整个效果每帧重建，而效果的清理函数正是**焦点
    // 归还**，重建一次就等于把焦点还回去一次。`modal` 决定装哪几个监听，必须在内。
    // `dialog` 在内：元素到场（或换了一个）时效果必须重跑，否则初始焦点与监听都挂在旧元素上。
  }, [enabled, closeOnEscape, modal, dialog]);

  return attach;
}
