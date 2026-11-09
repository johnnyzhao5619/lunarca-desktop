// 上下文菜单宿主（PRD-07-059）：全应用**唯一**的右键菜单开合点。
//
// 三件事收在这里，各表面因此只需声明"此刻这个对象上有哪些动作"：
// ① **原生菜单一律不出现**——webview 的默认菜单在开发构建里就是「Reload / Inspect
//    Element」，在打包构建里也只有浏览器语义的几项，二者都不是这个应用该给的答案；
// ② **落在文本输入面上一律给标准编辑菜单**（剪切/复制/粘贴/全选）——这条判定放在宿主
//    而不是各表面，于是"视图给了背景菜单、但用户右键的是工具栏搜索框"这类穿帮
//    不可能发生；
// ③ **开合、关闭、焦点归还、菜单归属行的高亮**只有一份实现。
//
// 剪贴板经 Rust 命令（`popup_copy` / `read_clipboard_text`）：webview 的
// `document.execCommand("paste")` 在 WKWebView 被禁，且剪贴板内容属敏感数据、
// 读写口径要与应用其余部分一致（TDD-12）。

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { commands } from "../ipc/bindings";
import { formatShortcut } from "@lunarca/editor-core/shortcut-display";
import {
  buildMenu,
  editableKindOf,
  isKeyboardInvocation,
  separator,
  textFieldMenuItems,
  type ContextMenuItem,
  type MenuPoint,
} from "../lib/context-menu";
import { ContextMenuPanel } from "./ui/ContextMenu";

/** 菜单归属元素在菜单打开期间挂的标记（CSS 据此给一层选中底色）。 */
const ACTIVE_TARGET_ATTR = "data-context-active";

export interface ContextMenuOpenOptions {
  /**
   * 菜单归属的元素（打开期间高亮）。缺省取 `event.currentTarget`——即挂了
   * `onContextMenu` 的那一行，正是用户心里"我右键的是它"的那个对象。
   *
   * **显式传 `null` = 不要高亮**：背景级菜单（视图空白处、图谱画布、编辑器整片
   * 编辑面）的 `currentTarget` 是整块容器，高亮它等于把半个界面刷上底色。
   */
  highlight?: HTMLElement | null;
  /** 无障碍名称（缺省为通用「上下文菜单」）。 */
  label?: string;
  /**
   * 右键落在输入框里时，把 `items` **接在**文本编辑四项之后，而不是让位给它们。只给「输入框
   * 本身就是某个对象的一格」的表面用（表格单元格：既要剪切 / 粘贴，也要插行删列）。
   */
  appendToTextField?: boolean;
}

export interface ContextMenuApi {
  /**
   * 打开菜单。`items` 为空即退回宿主兜底（文本编辑 / 选区复制 / 什么也不弹），
   * 但**原生菜单一定被拦下**。
   */
  open: (
    event: ReactMouseEvent | MouseEvent,
    items: ContextMenuItem[],
    options?: ContextMenuOpenOptions,
  ) => void;
  close: () => void;
}

const ContextMenuContext = createContext<ContextMenuApi | null>(null);

/** 取上下文菜单 API。宿主在 `main.tsx` 包住整棵树，故任意窗口的任意组件都可用。 */
export function useContextMenu(): ContextMenuApi {
  const api = useContext(ContextMenuContext);
  if (!api) {
    throw new Error("useContextMenu 必须在 <ContextMenuHost> 内使用");
  }
  return api;
}

interface MenuState {
  items: ContextMenuItem[];
  point: MenuPoint;
  label: string;
}

/** 文本输入面（`<input>` / `<textarea>`）。 */
type TextField = HTMLInputElement | HTMLTextAreaElement;

function isTextField(node: EventTarget | null): node is TextField {
  const tag = (node as HTMLElement | null)?.tagName?.toLowerCase();
  return tag === "input" || tag === "textarea";
}

/**
 * 以「原生 setter + input 事件」写回受控输入框。
 *
 * 直接赋 `field.value` 不会触发 React 的 `onChange`：界面上文字变了、状态还是旧的，
 * 下一次渲染又被打回去——剪切/粘贴会表现为"闪一下又还原"。
 */
function writeFieldValue(field: TextField, value: string, caret: number) {
  const prototype =
    field.tagName.toLowerCase() === "textarea"
      ? HTMLTextAreaElement.prototype
      : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(prototype, "value")?.set;
  if (setter) {
    setter.call(field, value);
  } else {
    field.value = value;
  }
  field.setSelectionRange(caret, caret);
  field.dispatchEvent(new Event("input", { bubbles: true }));
}

async function copyText(text: string) {
  if (text.length > 0) {
    await commands.popupCopy(text);
  }
}

async function readClipboard(): Promise<string> {
  const result = await commands.readClipboardText();
  return result.status === "ok" ? (result.data ?? "") : "";
}

export function ContextMenuHost({ children }: { children: ReactNode }) {
  const { t } = useTranslation("common");
  const [menu, setMenu] = useState<MenuState | null>(null);
  const highlightRef = useRef<HTMLElement | null>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);

  const close = useCallback(() => {
    setMenu(null);
  }, []);

  // 菜单关闭即摘掉归属行的高亮、把焦点还给原处（键盘用户不会掉到 body 上）。
  useEffect(() => {
    if (menu !== null) {
      return;
    }
    highlightRef.current?.removeAttribute(ACTIVE_TARGET_ATTR);
    highlightRef.current = null;
    const restore = restoreFocusRef.current;
    restoreFocusRef.current = null;
    // 只在**没人接手焦点**时才归还：菜单动作可能自己把焦点送去别处（「重命名」会
    // 打开内联输入框并聚焦），而 React 的子 effect 先于父 effect，无条件归还会把
    // 焦点从那个输入框抢回来——用户看到光标闪一下就没了。
    const claimed = document.activeElement !== null && document.activeElement !== document.body;
    if (restore?.isConnected && !claimed) {
      restore.focus({ preventScroll: true });
    }
  }, [menu]);

  const textFieldMenu = useCallback(
    (field: TextField): ContextMenuItem[] => {
      const start = field.selectionStart ?? 0;
      const end = field.selectionEnd ?? 0;
      const readOnly = field.readOnly;
      return textFieldMenuItems({
        labels: {
          cut: t("menu.cut"),
          copy: t("menu.copy"),
          paste: t("menu.paste"),
          selectAll: t("menu.selectAll"),
        },
        shortcuts: {
          cut: formatShortcut("cmd_or_ctrl+x"),
          copy: formatShortcut("cmd_or_ctrl+c"),
          paste: formatShortcut("cmd_or_ctrl+v"),
          selectAll: formatShortcut("cmd_or_ctrl+a"),
        },
        hasSelection: start !== end,
        readOnly,
        onCut: () => {
          void copyText(field.value.slice(start, end)).then(() => {
            writeFieldValue(field, field.value.slice(0, start) + field.value.slice(end), start);
            field.focus();
          });
        },
        onCopy: () => {
          void copyText(field.value.slice(start, end));
        },
        onPaste: () => {
          void readClipboard().then((text) => {
            if (!text) {
              return;
            }
            writeFieldValue(
              field,
              field.value.slice(0, start) + text + field.value.slice(end),
              start + text.length,
            );
            field.focus();
          });
        },
        onSelectAll: () => {
          field.focus();
          field.select();
        },
      });
    },
    [t],
  );

  /**
   * 富文本/只读区域的兜底：有选区就给「复制」，没有就什么也不弹。
   * （可编辑的 `contenteditable` 只有 CodeMirror 编辑器，它自带完整菜单。）
   */
  const selectionMenu = useCallback((): ContextMenuItem[] => {
    const selected = window.getSelection()?.toString() ?? "";
    if (selected.trim().length === 0) {
      return [];
    }
    return buildMenu([
      {
        kind: "action",
        id: "copy",
        label: t("menu.copy"),
        shortcut: formatShortcut("cmd_or_ctrl+c"),
        onSelect: () => {
          void copyText(selected);
        },
      },
    ]);
  }, [t]);

  const open = useCallback<ContextMenuApi["open"]>(
    (event, items, options) => {
      // **最内层的表面先认领，外层自动让位**：行菜单与所在视图的背景菜单都挂着
      // `onContextMenu`，事件冒泡会让外层紧跟着把内层的菜单顶掉。判据放在这里一次，
      // 各视图的背景菜单就不必逐个写"如果孩子已经处理过就返回"。
      if (event.defaultPrevented) {
        return;
      }
      event.preventDefault();
      const target = event.target as HTMLElement | null;
      // 文本输入面上的右键**永远**是编辑菜单：表面自己的背景菜单在此让位，
      // 否则「回收站页的搜索框」会弹出「清空回收站」这种驴唇不对马嘴的菜单。
      const editable = editableKindOf(target);
      const resolved =
        editable !== null && isTextField(target)
          ? options?.appendToTextField && items.length > 0
            ? buildMenu([...textFieldMenu(target), separator("surface"), ...items])
            : textFieldMenu(target)
          : items.length > 0
            ? items
            : selectionMenu();
      if (resolved.length === 0) {
        // 无可用动作就不弹空壳；原生菜单已被上面的 preventDefault 拦下。
        setMenu(null);
        return;
      }
      const native = "nativeEvent" in event ? event.nativeEvent : event;
      // 兜底路径由 document 上的监听触发，`currentTarget` 是 document 而非元素——
      // 拿它去 setAttribute 会当场抛异常，故只认真正的元素。
      const currentTarget = event.currentTarget instanceof HTMLElement ? event.currentTarget : null;
      const anchor = currentTarget ?? target;
      const point = isKeyboardInvocation(native)
        ? anchorFromElement(target ?? anchor)
        : { x: native.clientX, y: native.clientY };

      highlightRef.current?.removeAttribute(ACTIVE_TARGET_ATTR);
      // 「没传」取 currentTarget，「显式传 null」= 不高亮，二者必须区分开。
      const highlight = options && "highlight" in options ? options.highlight : anchor;
      highlight?.setAttribute(ACTIVE_TARGET_ATTR, "true");
      highlightRef.current = highlight ?? null;
      restoreFocusRef.current =
        document.activeElement instanceof HTMLElement ? document.activeElement : null;

      setMenu({ items: resolved, point, label: options?.label ?? t("menu.contextMenu") });
    },
    [selectionMenu, t, textFieldMenu],
  );

  const api = useMemo<ContextMenuApi>(() => ({ open, close }), [close, open]);

  // 未被任何表面认领的右键：原生菜单一律拦下，再走宿主兜底。
  useEffect(() => {
    function onContextMenu(event: MouseEvent) {
      if (event.defaultPrevented) {
        return;
      }
      api.open(event, []);
    }
    document.addEventListener("contextmenu", onContextMenu);
    return () => {
      document.removeEventListener("contextmenu", onContextMenu);
    };
  }, [api]);

  // 菜单开着时的全局关闭条件（Esc 在面板内处理，焦点就在面板上）。
  useEffect(() => {
    if (menu === null) {
      return;
    }
    const dismiss = () => {
      close();
    };
    window.addEventListener("mousedown", dismiss);
    window.addEventListener("blur", dismiss);
    window.addEventListener("resize", dismiss);
    // 捕获阶段：内层滚动容器滚动时菜单会与目标行脱节，一并关闭。
    window.addEventListener("scroll", dismiss, true);
    return () => {
      window.removeEventListener("mousedown", dismiss);
      window.removeEventListener("blur", dismiss);
      window.removeEventListener("resize", dismiss);
      window.removeEventListener("scroll", dismiss, true);
    };
  }, [menu, close]);

  // 落位入参随菜单状态记忆：每次渲染新建对象会让面板的测量 effect 无谓重跑。
  const placement = useMemo(
    () => (menu ? ({ kind: "anchor", point: menu.point } as const) : null),
    [menu],
  );

  return (
    <ContextMenuContext.Provider value={api}>
      {children}
      {menu && placement
        ? createPortal(
            <ContextMenuPanel
              items={menu.items}
              placement={placement}
              onDismiss={close}
              onCloseSelf={close}
              label={menu.label}
              autoFocus
            />,
            document.body,
          )
        : null}
    </ContextMenuContext.Provider>
  );
}

/** 键盘唤起（`Shift+F10` / 菜单键）时锚到目标元素左下角。 */
function anchorFromElement(element: HTMLElement | null): MenuPoint {
  if (!element) {
    return { x: 0, y: 0 };
  }
  const rect = element.getBoundingClientRect();
  return { x: rect.left, y: rect.bottom };
}
