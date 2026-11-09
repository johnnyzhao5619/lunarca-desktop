/**
 * 指针事件拖拽基元（PRD-06-006 树内移动 / PRD-07-035 标签重排共用）。
 *
 * **为什么不用 HTML5 Drag and Drop**：Tauri 的 OS 文件拖放（`dragDropEnabled`）与 webview 的
 * HTML5 DnD **互斥**——早先为保住树内拖拽移动而关闭前者，代价是拖入文件导入做不了
 * （PRD-05-001）。指针事件与 OS 拖放**不冲突**，改用它后二者兼得。
 *
 * 顺带的收益（与 Tauri 无关）：HTML5 DnD 的拖影是浏览器生成的半透明截图、不可定制，
 * 触屏也不支持；指针事件下拖影是我们自己的 DOM，落点指示、吸附、动效都能控制。
 * Obsidian / Notion / Linear 的树与看板拖拽同样不用 HTML5 DnD。
 *
 * 落点解析统一走 `data-drop-id`：调用方只需给可放置元素加该属性，无须各自写
 * `pointermove` 命中测试（DRY——两个消费方共用同一份实现）。
 */

import { useCallback, useEffect, useRef, useState } from "react";
// 起拖阈值与编辑器表格把手共用一处（`@lunarca/editor-core/drag-threshold`）。
import { shouldStartDrag } from "@lunarca/editor-core/drag-threshold";

/** 拖影相对指针的偏移（px）：不遮住指针下的落点，各消费方同一个值。 */
export const DRAG_GHOST_OFFSET_PX = 12;

/** 可放置元素的标记属性；值即落点 id（空串合法，表示"根"）。 */
export const DROP_ID_ATTR = "data-drop-id";

/**
 * 从屏幕坐标解析落点 id：取该点最上层元素，向上找最近的 `data-drop-id`。
 * 无命中返回 `null`（调用方据此不高亮、松手不触发）。
 */
export function resolveDropId(clientX: number, clientY: number): string | null {
  const element = document.elementFromPoint(clientX, clientY);
  const zone = element?.closest(`[${DROP_ID_ATTR}]`);
  return zone?.getAttribute(DROP_ID_ATTR) ?? null;
}

export interface PointerDragHandle<T> {
  /** 正在拖拽的载荷（`null` = 未拖拽）。 */
  payload: T | null;
  /** 拖影当前坐标（视口坐标；`payload` 为 null 时无意义）。 */
  position: { x: number; y: number };
  /** 当前悬停的落点 id（`null` = 不可放）。 */
  dropId: string | null;
  /** 在候选源元素的 `onPointerDown` 里调用；未达阈值不会启动拖拽。 */
  begin: (event: React.PointerEvent, payload: T) => void;
}

/**
 * 指针拖拽状态机。松手且落在合法落点上时调用 `onDrop`；不在任何落点上时交给 `onDropOutside`
 * （可选：例如文件树条目拖进编辑器正文插链接），没给就是取消。ESC 即取消。
 *
 * 监听挂在 `window` 上而非源元素：拖拽过程中指针可能移出源元素（这正是拖拽的常态），
 * 挂在源元素上会在越界瞬间丢事件。
 */
export function usePointerDrag<T>(
  onDrop: (payload: T, dropId: string) => void,
  onDropOutside?: (payload: T, point: { x: number; y: number }) => void,
): PointerDragHandle<T> {
  const [payload, setPayload] = useState<T | null>(null);
  const [position, setPosition] = useState({ x: 0, y: 0 });
  const [dropId, setDropId] = useState<string | null>(null);
  /** 待定拖拽：已按下但位移未达阈值——此时仍可能只是一次点击。 */
  const pending = useRef<{ payload: T; x: number; y: number } | null>(null);
  const onDropRef = useRef(onDrop);
  onDropRef.current = onDrop;
  const onDropOutsideRef = useRef(onDropOutside);
  onDropOutsideRef.current = onDropOutside;

  const reset = useCallback(() => {
    pending.current = null;
    setPayload(null);
    setDropId(null);
  }, []);

  useEffect(() => {
    function onMove(event: PointerEvent) {
      const start = pending.current;
      if (start && !payload) {
        if (!shouldStartDrag(event.clientX - start.x, event.clientY - start.y)) {
          return;
        }
        setPayload(start.payload);
      }
      if (!pending.current) {
        return;
      }
      setPosition({ x: event.clientX, y: event.clientY });
      setDropId(resolveDropId(event.clientX, event.clientY));
    }
    function onUp(event: PointerEvent) {
      const start = pending.current;
      // 未达阈值 = 用户只是点了一下，交还给 click 处理，绝不当作拖拽。
      if (start && payload) {
        const target = resolveDropId(event.clientX, event.clientY);
        if (target !== null) {
          onDropRef.current(start.payload, target);
        } else {
          onDropOutsideRef.current?.(start.payload, { x: event.clientX, y: event.clientY });
        }
      }
      reset();
    }
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape") {
        reset();
      }
    }
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", reset);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", reset);
      window.removeEventListener("keydown", onKey);
    };
  }, [payload, reset]);

  const begin = useCallback((event: React.PointerEvent, next: T) => {
    // 只响应主按钮；右键/中键有各自语义（上下文菜单 / 新标签页打开）。
    if (event.button !== 0) {
      return;
    }
    pending.current = { payload: next, x: event.clientX, y: event.clientY };
    setPosition({ x: event.clientX, y: event.clientY });
  }, []);

  return { payload, position, dropId, begin };
}
