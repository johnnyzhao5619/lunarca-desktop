import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "../../ipc/bindings";
import { cssVariables } from "../../lib/css-variables";
import { vaultErrorText } from "../../lib/vault-error";

declare global {
  interface Window {
    /** Rust 显示面板时 eval：重置框选状态；`hasCursor` = 本面板所在屏当前有光标（立即种提示条）。 */
    __lunaScreenshotReset?: (hasCursor: boolean) => void;
  }
}

/** 低于该边长（CSS 逻辑像素）的拖选视为误触：回到待选态可重拖，不提交（与 Rust 防线同值语义）。 */
const MIN_SELECTION_SIZE = 4;

type Point = { x: number; y: number };

type SelectionRect = { x: number; y: number; width: number; height: number };

/** 八个调整把手（四角 + 四边），名字即它管哪几条边。 */
const HANDLES = ["tl", "t", "tr", "r", "br", "b", "bl", "l"] as const;
type Handle = (typeof HANDLES)[number];

/** 一次按下之后在做什么：画新框 / 整体移动 / 拖某个把手改大小。 */
type Drag =
  | { kind: "draw"; start: Point }
  | { kind: "move"; origin: Point; base: SelectionRect }
  | { kind: "resize"; handle: Handle; base: SelectionRect };

/** 拖把手：把手所管的那几条边跟着指针走，其余不动；越过对边时自然翻转。 */
export function resizeRect(base: SelectionRect, handle: Handle, point: Point): SelectionRect {
  let left = base.x;
  let top = base.y;
  let right = base.x + base.width;
  let bottom = base.y + base.height;
  if (handle.includes("l")) left = point.x;
  if (handle.includes("r")) right = point.x;
  if (handle.includes("t")) top = point.y;
  if (handle.includes("b")) bottom = point.y;
  return rectFrom({ x: left, y: top }, { x: right, y: bottom });
}

/** 整体移动：位移量照搬，但整个框不出本屏视口。 */
export function moveRect(base: SelectionRect, origin: Point, point: Point): SelectionRect {
  const x = Math.min(Math.max(base.x + point.x - origin.x, 0), window.innerWidth - base.width);
  const y = Math.min(Math.max(base.y + point.y - origin.y, 0), window.innerHeight - base.height);
  return { ...base, x, y };
}

function rectFrom(a: Point, b: Point): SelectionRect {
  return {
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    width: Math.abs(a.x - b.x),
    height: Math.abs(a.y - b.y),
  };
}

/** 拖选点钳制在本屏视口内：pointer capture 下指针可越屏，选区不得溢出本面板所在屏。 */
function clampToViewport(x: number, y: number): Point {
  return {
    x: Math.min(Math.max(x, 0), window.innerWidth),
    y: Math.min(Math.max(y, 0), window.innerHeight),
  };
}

/**
 * 截屏翻译框选层（PRD-01-007 / TDD-05 §1.5，对标 Snipaste / CleanShot）。
 *
 * 纯视图（ADR-003）：只跟踪指针、上报选区；权限/截屏/OCR/进弹窗全在 Rust。
 * 多屏：每个显示器一个本组件实例（各自面板窗口），拖选限定单屏（macOS 惯例，同 ⇧⌘4）；
 * 提示条只在光标所在屏显示（显示时由 Rust 种子，随指针跨屏迁移）。
 *
 * 交互（ADR-137 H4）：移动显示十字线 + 提示条 → 按住拖出选区 → **松手进入可调整态**：
 * 八个把手可拖、框内可整体拖动、框外再拖 = 重画；框旁一条迷你工具条「翻译 / 取消」，
 * 双击框内 = 翻译。此前松手即提交，角点画着把手却拖不动——差一点框歪了只能取消重来。
 * 右键随时取消（Esc 由 Rust CGEventTap 收口）。
 */
export function ScreenshotOverlay() {
  const { t } = useTranslation("popup");
  const [cursor, setCursor] = useState<Point | null>(null);
  const [rect, setRect] = useState<SelectionRect | null>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  /** 松手后可调整（未提交）。 */
  const [adjusting, setAdjusting] = useState(false);
  // 显示瞬间光标是否在本屏：指针尚未移动时也能立刻给出提示条（之后随指针进出更新）。
  const [hintSeeded, setHintSeeded] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  // 提交后冻结 UI（隐藏窗口由 Rust 负责），防止重复提交/闪烁。
  const submittedRef = useRef(false);

  const reset = useCallback((hasCursor: boolean) => {
    submittedRef.current = false;
    setCursor(null);
    setRect(null);
    setDrag(null);
    setAdjusting(false);
    setHintSeeded(hasCursor);
    setSubmitError(null);
  }, []);

  useEffect(() => {
    window.__lunaScreenshotReset = reset;
    return () => {
      delete window.__lunaScreenshotReset;
    };
  }, [reset]);

  const confirm = useCallback(async (selection: SelectionRect) => {
    if (submittedRef.current) return;
    submittedRef.current = true;
    try {
      const result = await commands.screenshotOverlayConfirm(
        selection.x,
        selection.y,
        selection.width,
        selection.height,
      );
      if (result.status === "ok") return;
      submittedRef.current = false;
      setSubmitError(vaultErrorText(result.error));
    } catch (error) {
      submittedRef.current = false;
      setSubmitError(vaultErrorText(error instanceof Error ? error.message : String(error)));
    }
  }, []);

  const cancel = useCallback(() => {
    if (submittedRef.current) return;
    submittedRef.current = true;
    void commands.screenshotOverlayCancel();
  }, []);

  // 键盘兜底：面板通常非 key 窗口收不到键事件（Esc 主路径在 Rust CGEventTap），
  // 若确实获得键焦点则就地处理——Esc 取消、回车确认可调整态的框，双保险零成本。
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") cancel();
      if (e.key === "Enter" && adjusting && rect) void confirm(rect);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [adjusting, cancel, confirm, rect]);

  function handlePointerDown(e: React.PointerEvent<HTMLDivElement>) {
    if (submittedRef.current || e.button !== 0) return; // 右键走 onContextMenu 取消
    const target = e.target instanceof Element ? e.target : null;
    if (target?.closest(".luna-shot__toolbar")) return; // 工具条上的按钮自己处理
    e.currentTarget.setPointerCapture(e.pointerId);
    const point = clampToViewport(e.clientX, e.clientY);
    setSubmitError(null);
    const handle = target?.closest("[data-handle]")?.getAttribute("data-handle") as Handle | null;
    if (adjusting && rect && handle) {
      setDrag({ kind: "resize", handle, base: rect });
      return;
    }
    if (adjusting && rect && target?.closest(".luna-shot__selection")) {
      setDrag({ kind: "move", origin: point, base: rect });
      return;
    }
    // 框外按下 = 重画。
    setAdjusting(false);
    setRect({ x: point.x, y: point.y, width: 0, height: 0 });
    setDrag({ kind: "draw", start: point });
  }

  function handlePointerMove(e: React.PointerEvent<HTMLDivElement>) {
    if (submittedRef.current) return;
    const point = clampToViewport(e.clientX, e.clientY);
    setCursor(point);
    if (!drag) return;
    if (drag.kind === "draw") setRect(rectFrom(drag.start, point));
    if (drag.kind === "move") setRect(moveRect(drag.base, drag.origin, point));
    if (drag.kind === "resize") setRect(resizeRect(drag.base, drag.handle, point));
  }

  function handlePointerLeave() {
    if (submittedRef.current || drag || adjusting) return; // 拖选中指针受 capture 管辖
    setCursor(null);
    setHintSeeded(false); // 指针去了别的屏：十字线与提示条随之迁移
  }

  function handlePointerUp(e: React.PointerEvent<HTMLDivElement>) {
    if (submittedRef.current || e.button !== 0 || !drag) return;
    setDrag(null);
    if (!rect || rect.width < MIN_SELECTION_SIZE || rect.height < MIN_SELECTION_SIZE) {
      // 误触/单击：回到待选态可重拖（Snipaste 同语义），不提交。
      setRect(null);
      setAdjusting(false);
      return;
    }
    setAdjusting(true);
  }

  function handleContextMenu(e: React.MouseEvent<HTMLDivElement>) {
    e.preventDefault();
    cancel();
  }

  const selection = rect && (rect.width > 0 || rect.height > 0) ? rect : null;
  // 尺寸徽标显示**实际物理像素**（Snipaste 惯例；OCR 清晰度以物理像素为准）。
  const dpr = window.devicePixelRatio || 1;
  const badge = selection
    ? `${Math.round(selection.width * dpr)} × ${Math.round(selection.height * dpr)}`
    : null;
  // 徽标默认在选区左上角外侧；贴近屏幕顶部时翻到选区内侧，保证可见。
  const badgeAbove = selection ? selection.y > 28 : false;
  // 工具条默认在选区下方右对齐；下方放不下就翻到框内底部。
  const toolbarBelow = selection ? selection.y + selection.height + 48 < window.innerHeight : true;

  return (
    <div
      className="luna-shot"
      data-selecting={selection !== null}
      data-adjusting={adjusting || undefined}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
      onPointerLeave={handlePointerLeave}
      onContextMenu={handleContextMenu}
    >
      {selection === null && cursor && (
        <>
          <div
            className="luna-shot__crosshair luna-shot__crosshair--v"
            style={cssVariables({ "--shot-cursor-x": `${cursor.x}px` })}
          />
          <div
            className="luna-shot__crosshair luna-shot__crosshair--h"
            style={cssVariables({ "--shot-cursor-y": `${cursor.y}px` })}
          />
        </>
      )}
      {selection === null && (hintSeeded || cursor !== null) && (
        <div className="luna-shot__hint">{t("overlay.hint")}</div>
      )}
      {submitError ? (
        <div className="luna-shot__error" role="alert">
          <strong>{t("overlay.failed")}</strong>
          <span>{submitError}</span>
          <span>{t("overlay.retry")}</span>
        </div>
      ) : null}
      {selection && (
        <div
          className="luna-shot__selection"
          style={cssVariables({
            "--shot-selection-x": `${selection.x}px`,
            "--shot-selection-y": `${selection.y}px`,
            "--shot-selection-width": `${selection.width}px`,
            "--shot-selection-height": `${selection.height}px`,
          })}
          onDoubleClick={() => {
            if (adjusting) void confirm(selection);
          }}
        >
          <span className="luna-shot__badge" data-position={badgeAbove ? "above" : "inside"}>
            {badge}
          </span>
          {HANDLES.map((handle) => (
            <span key={handle} className="luna-shot__handle" data-handle={handle} />
          ))}
          {adjusting && !drag ? (
            <div className="luna-shot__toolbar" data-position={toolbarBelow ? "below" : "inside"}>
              <button
                type="button"
                className="luna-shot__tool"
                data-variant="primary"
                onClick={() => void confirm(selection)}
              >
                {t("overlay.translate")}
              </button>
              <button type="button" className="luna-shot__tool" onClick={cancel}>
                {t("overlay.cancel")}
              </button>
            </div>
          ) : null}
        </div>
      )}
    </div>
  );
}
