import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { useTranslation } from "react-i18next";
import { CameraIcon, CheckIcon, CloseIcon, CropIcon } from "../../components/ui/Icon";
import { IconButton } from "../../components/ui/IconButton";
import { commands } from "../../ipc/bindings";
import { vaultErrorText } from "../../lib/vault-error";

declare global {
  interface Window {
    __lunaDesktopLensSetEditing?: (editing: boolean) => void;
  }
}

/**
 * 取景框最小尺寸（逻辑点）。与 Rust `DESKTOP_LENS_MIN_WIDTH_POINTS` / `..._HEIGHT_POINTS`
 * 同口径；Rust 落盘时权威夹紧，这里只为拖拽手感不越界。
 */
const LENS_MIN_WIDTH_POINTS = 160;
const LENS_MIN_HEIGHT_POINTS = 100;

/** 当前窗口的逻辑点几何。译境几何全程走逻辑点（单位铁律见 Rust `show_desktop_lens`）。 */
async function currentLogicalGeometry() {
  const appWindow = getCurrentWindow();
  const scale = await appWindow.scaleFactor();
  const [position, size] = await Promise.all([appWindow.outerPosition(), appWindow.outerSize()]);
  return { position: position.toLogical(scale), size: size.toLogical(scale) };
}

/**
 * 上报当前窗口几何并退出调整态。写盘成败与退出调整态由 Rust 单点决定（写不下也照常退出，
 * 不把用户困在面板里，并就地发通知说明位置不会被记住），故此处**不再**额外调一次退出。
 */
async function persistGeometryAndExitEditing() {
  const { position, size } = await currentLogicalGeometry();
  await commands.desktopLensSaveGeometry(
    Math.round(position.x),
    Math.round(position.y),
    Math.round(size.width),
    Math.round(size.height),
  );
}

/** 译境的可视取景外框；采集/OCR/翻译与位置持久化均在 Rust。 */
export function DesktopLensView() {
  const { t } = useTranslation("popup");
  const [editing, setEditing] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const resizeStart = useRef<{ x: number; y: number; width: number; height: number } | null>(null);

  const setEditingFromHost = useCallback((next: boolean) => setEditing(next), []);

  useEffect(() => {
    window.__lunaDesktopLensSetEditing = setEditingFromHost;
    return () => {
      delete window.__lunaDesktopLensSetEditing;
    };
  }, [setEditingFromHost]);

  async function close() {
    const result = await commands.desktopLensHide();
    if (result.status === "error") {
      setActionError(vaultErrorText(result.error));
    }
  }

  async function beginResize(event: React.PointerEvent<HTMLButtonElement>) {
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    const { size } = await currentLogicalGeometry();
    resizeStart.current = {
      x: event.screenX,
      y: event.screenY,
      width: size.width,
      height: size.height,
    };
  }

  function resize(event: React.PointerEvent<HTMLButtonElement>) {
    const start = resizeStart.current;
    if (!start) return;
    // 指针坐标与起始尺寸同为逻辑点，故直接相加——不再乘 devicePixelRatio（那正是 Retina 上
    // 缩放与实际框体对不上的根源）。
    const width = Math.max(
      LENS_MIN_WIDTH_POINTS,
      Math.round(start.width + (event.screenX - start.x)),
    );
    const height = Math.max(
      LENS_MIN_HEIGHT_POINTS,
      Math.round(start.height + (event.screenY - start.y)),
    );
    void getCurrentWindow().setSize(new LogicalSize(width, height));
  }

  function endResize() {
    resizeStart.current = null;
  }

  useEffect(() => {
    if (!editing) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      // Esc 仅退出调整态，避免用户在移动/缩放过程中误杀整个镜头会话。
      if (event.key === "Escape") {
        event.preventDefault();
        void persistGeometryAndExitEditing();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [editing]);

  return (
    <section
      className="luna-desktop-lens"
      data-editing={editing}
      aria-label={t("desktopLens.frameAria")}
    >
      {editing && (
        <>
          <header
            className="luna-desktop-lens__toolbar"
            onPointerDown={(event) => {
              if (
                event.button === 0 &&
                !(event.target instanceof Element && event.target.closest("[data-no-drag]"))
              ) {
                void getCurrentWindow().startDragging();
              }
            }}
          >
            <span>{t("desktopLens.editTitle")}</span>
            <div className="luna-desktop-lens__actions" data-no-drag>
              {/* 按钮走原语（ADR-137 H8 / ADR-111 ③）：此前 `.luna-desktop-lens__actions button`
                  一条元素选择器给两颗按钮上皮、再用 `:first-child` 认出主按钮。 */}
              <button
                type="button"
                className="luna-btn luna-btn--primary luna-btn--sm"
                onClick={() => void persistGeometryAndExitEditing()}
              >
                <CheckIcon size={14} />
                <span>{t("desktopLens.done")}</span>
              </button>
              <button
                type="button"
                className="luna-btn luna-btn--secondary luna-btn--sm"
                onClick={() => void close()}
              >
                <CloseIcon size={14} />
                <span>{t("desktopLens.close")}</span>
              </button>
            </div>
          </header>
          {actionError && (
            <p className="luna-desktop-lens__error" role="alert">
              {actionError}
            </p>
          )}
          <button
            type="button"
            className="luna-desktop-lens__resize"
            aria-label={t("desktopLens.resize")}
            onPointerDown={(event) => void beginResize(event)}
            onPointerMove={resize}
            onPointerUp={endResize}
          />
        </>
      )}
    </section>
  );
}

/**
 * 贴在镜框外侧上缘的紧凑控制条，恰好三枚键——这是完备且最小的集合：
 * **捕捉**（会话中唯一会反复点的动作）、**调整**（改取景范围）、**关闭**（退出会话）。
 *
 * 刻意**不**放复制/存入笔记/切换语言：那些是译文产出后的动作，归翻译弹窗的结果操作条
 * （`PRD-01-021`），搬到这里会造出第二条并行动作链；也不放重新识别（再点一次相机即可）、
 * 暂停与历史（前者非本切片能力，后者违反"不建立浏览历史"的隐私契约）。控制条常驻在用户
 * 正在阅读的内容之上，每多一枚图标都是永久视觉噪声。
 */
export function DesktopLensTrigger() {
  const { t } = useTranslation("popup");
  const [capturing, setCapturing] = useState(false);
  const [captureError, setCaptureError] = useState<string | null>(null);

  async function capture() {
    setCapturing(true);
    setCaptureError(null);
    try {
      const result = await commands.desktopLensCapture();
      // 控制条只有图标键、放不下错误文案，故以描边告警 + tooltip 如实呈现，绝不静默吞掉。
      // 用户可执行的失败（跨屏取景、OCR 空、权限缺失）另经通知中心给出完整修复指引。
      if (result.status === "error") {
        setCaptureError(vaultErrorText(result.error));
      }
    } finally {
      setCapturing(false);
    }
  }

  const captureLabel = capturing ? t("desktopLens.capturing") : t("desktopLens.capture");

  return (
    <div
      className="luna-desktop-lens-trigger"
      aria-label={t("desktopLens.controlsAria")}
      data-error={captureError !== null}
    >
      {/* 三枚键走 `IconButton` 原语（ADR-137 H8）：此前 `.luna-desktop-lens-trigger button` 元素
          兜底给它们上皮。采集失败时可访问名就是那句原因（悬停即见，失败不以「没反应」收场）。 */}
      <IconButton
        className="luna-desktop-lens-trigger__capture"
        label={captureError ?? captureLabel}
        icon={<CameraIcon size={16} />}
        disabled={capturing}
        onClick={() => void capture()}
      />
      {/* 分隔：左侧是会反复点的主动作，右侧是偶发的会话控制——也让「关闭」离「捕捉」最远，
          降低误点代价（误点关闭要重新开会话并重新取景）。 */}
      <span className="luna-desktop-lens-trigger__divider" aria-hidden="true" />
      <IconButton
        className="luna-desktop-lens-trigger__edit"
        label={t("desktopLens.adjust")}
        icon={<CropIcon size={16} />}
        onClick={() => void commands.desktopLensSetEditing(true)}
      />
      <IconButton
        className="luna-desktop-lens-trigger__close"
        label={t("desktopLens.close")}
        icon={<CloseIcon size={16} />}
        onClick={() => void commands.desktopLensHide()}
      />
    </div>
  );
}
