import { useCallback, useRef, type KeyboardEvent } from "react";

/**
 * 标签组（`role="tablist"`）的键盘导航——WAI-ARIA APG 的 tabs 模式，全应用唯一实现。
 *
 * **声明了 `role="tab"` 就要兑现键盘行为**：收编前只有模型管理面板自己写了一份方向键
 * + roving tabindex，其余标签组都只有 `role="tab"` 这层声明——读屏念得出"标签 2/4"，但
 * Tab 键进不去、方向键不动。声明了角色却没有配套行为，比不声明更糟：它对辅助技术承诺了
 * 一套交互，然后不兑现。这条如今由 `keyboard-reachability-guard.test.ts` 机器守住。
 *
 * APG 规定的是：**整组只有一个 Tab 停靠点**（选中项 `tabIndex=0`，其余 `-1`），组内用
 * ←/→ 循环移动、Home/End 跳两端，移动即选中（本应用的标签都是轻量切换，无需 manual
 * activation 那一档）。这三件事写在一起才成立，所以做成一个 hook 而不是三个工具函数。
 */
export interface TablistNavigation<T extends string> {
  /** 挂在每个 `role="tab"` 元素上，供方向键把焦点交出去。 */
  register: (value: T) => (element: HTMLElement | null) => void;
  /** 该标签此刻该不该是这一组的 Tab 停靠点。 */
  tabIndexOf: (value: T) => 0 | -1;
  /** 挂在每个 `role="tab"` 的 `onKeyDown` 上。 */
  onKeyDown: (event: KeyboardEvent) => void;
}

export function useTablistNavigation<T extends string>(
  values: readonly T[],
  active: T,
  onSelect: (value: T) => void,
): TablistNavigation<T> {
  const refs = useRef<Partial<Record<T, HTMLElement | null>>>({});
  // 事件处理器里读到的必须是**这一帧**的 values/active/onSelect：把它们存进 ref，
  // 回调本身就不必随渲染重建，挂在几十个标签上也不会每帧换一个函数。
  const latest = useRef({ values, active, onSelect });
  latest.current = { values, active, onSelect };

  const register = useCallback(
    (value: T) => (element: HTMLElement | null) => {
      refs.current[value] = element;
    },
    [],
  );

  const tabIndexOf = useCallback((value: T): 0 | -1 => (value === active ? 0 : -1), [active]);

  const onKeyDown = useCallback((event: KeyboardEvent) => {
    const { values: list, active: current, onSelect: select } = latest.current;
    const index = list.indexOf(current);
    if (index < 0 || list.length === 0) {
      return;
    }
    const next =
      event.key === "ArrowRight" || event.key === "ArrowDown"
        ? list[(index + 1) % list.length]
        : event.key === "ArrowLeft" || event.key === "ArrowUp"
          ? list[(index - 1 + list.length) % list.length]
          : event.key === "Home"
            ? list[0]
            : event.key === "End"
              ? list[list.length - 1]
              : null;
    if (next === null || next === undefined) {
      return;
    }
    event.preventDefault();
    select(next);
    refs.current[next]?.focus();
  }, []);

  return { register, tabIndexOf, onKeyDown };
}
