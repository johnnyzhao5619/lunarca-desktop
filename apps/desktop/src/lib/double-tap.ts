/**
 * 应用内快捷键的双击判定（与 Rust `platform::DoubleTapDetector` 同一算法）。
 *
 * 全局键的双击由系统事件 tap 在 Rust 判；应用内键（快速切换器等）只在 webview 里有事件，
 * 故这里再实现一份**同样的规则**：相邻两次同一组合间隔 ≤ 时间窗即命中并复位，中间夹了
 * 别的键则作废。时间窗与全局键共用 `shortcuts.double_tap_window_ms`，不另设默认。
 */
export class DoubleTapDetector {
  private lastId: string | null = null;
  private lastMs = 0;

  /** 记录一次命中的组合按下；返回 `true` = 本次完成了一次双击。 */
  record(id: string, nowMs: number, windowMs: number): boolean {
    if (this.lastId === id && nowMs - this.lastMs <= windowMs) {
      this.reset();
      return true;
    }
    this.lastId = id;
    this.lastMs = nowMs;
    return false;
  }

  /** 中间夹了别的键：已开始的那一对不再成立。 */
  reset(): void {
    this.lastId = null;
    this.lastMs = 0;
  }
}
