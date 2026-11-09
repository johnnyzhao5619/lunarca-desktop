import { useEffect, useRef } from "react";

/** 算作「在用」的输入（只看有没有，不读内容）。 */
const ACTIVITY_EVENTS = ["pointerdown", "pointermove", "keydown", "wheel", "touchstart", "scroll"] as const;
/** 后台标签页里计时器会被节流：除了到点的计时器，另按这个间隔核对一次「上次活动」到现在多久了。 */
const CHECK_INTERVAL_MS = 15_000;

/**
 * 空闲 `ms` 毫秒后调一次 `onIdle`（`ms` 为 `null` 时不计时）。判据是「上次活动的时刻」而不是
 * 一个一直在被重置的计时器：标签页切到后台、电脑睡眠之后回来，过了时长的立即锁定。
 */
export function useIdleTimer(ms: number | null, onIdle: () => void): void {
  const callback = useRef(onIdle);
  callback.current = onIdle;

  useEffect(() => {
    if (ms === null) return;
    let last = Date.now();
    let fired = false;
    const check = () => {
      if (!fired && Date.now() - last >= ms) {
        fired = true;
        callback.current();
      }
    };
    const touch = () => {
      check();
      last = Date.now();
    };
    for (const name of ACTIVITY_EVENTS) window.addEventListener(name, touch, { passive: true, capture: true });
    document.addEventListener("visibilitychange", check);
    const interval = window.setInterval(check, Math.min(CHECK_INTERVAL_MS, ms));
    return () => {
      for (const name of ACTIVITY_EVENTS) window.removeEventListener(name, touch, { capture: true });
      document.removeEventListener("visibilitychange", check);
      window.clearInterval(interval);
    };
  }, [ms]);
}
