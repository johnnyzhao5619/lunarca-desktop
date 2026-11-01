import { useLayoutEffect, useState, type RefObject } from "react";

/**
 * 观察一个元素的内联尺寸（`inline-size`，CSS 逻辑宽度）。
 *
 * 用 `useLayoutEffect` 而不是 `useEffect`：首帧要在**绘制前**拿到真实宽度，否则以缺省档
 * 画一帧再跳到正确档，用户会看到页头闪一下。`ResizeObserver` 负责其后的变化
 * （分屏拖宽、侧栏收起、窗口缩放），回调里只写 state，比较由 React 去重。
 *
 * 返回 `null` 表示还没有元素可量（ref 未挂载）。
 */
export function useInlineSize(ref: RefObject<HTMLElement | null>): number | null {
  const [size, setSize] = useState<number | null>(null);

  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) {
      return;
    }
    const measure = () => {
      setSize(element.getBoundingClientRect().width);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => {
      observer.disconnect();
    };
  }, [ref]);

  return size;
}

/**
 * 元素的内容是否在横向溢出（`scrollWidth > clientWidth`）。
 *
 * 除了元素自身尺寸变化，**内容变化**（多开一个标签）也会改变溢出与否，而 ResizeObserver
 * 只看盒子不看内容，故调用方把会改变内容的依据（如标签数）作为 `contentKey` 传入，
 * 变化时重新量一次。
 */
export function useInlineOverflow(
  ref: RefObject<HTMLElement | null>,
  contentKey: unknown,
): boolean {
  const [overflowing, setOverflowing] = useState(false);

  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) {
      return;
    }
    const measure = () => {
      setOverflowing(element.scrollWidth > element.clientWidth + 1);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => {
      observer.disconnect();
    };
  }, [ref, contentKey]);

  return overflowing;
}

/** 元素的内联与块向尺寸（逻辑宽高）。 */
export interface BoxSize {
  inline: number;
  block: number;
}

/**
 * 观察一个元素的**两向**尺寸。锚定浮层（AI 面板 / 选区气泡）翻转判定要的是真实高度：
 * 此前用一个写死的估算值，面板内容一变（结果区、搜索列表）就判错方向，底部溢出。
 * 与 `useInlineSize` 同一取法（首帧 layout effect + ResizeObserver）。
 */
export function useBoxSize(ref: RefObject<HTMLElement | null>): BoxSize | null {
  const [size, setSize] = useState<BoxSize | null>(null);

  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) {
      return;
    }
    const measure = () => {
      const rect = element.getBoundingClientRect();
      setSize((current) =>
        current && current.inline === rect.width && current.block === rect.height
          ? current
          : { inline: rect.width, block: rect.height },
      );
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => {
      observer.disconnect();
    };
  }, [ref]);

  return size;
}
