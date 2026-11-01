/**
 * 模块级共享订阅的**按需取值**（选择器）。
 *
 * 同步总览、处理队列这类广播一拍就换掉整份快照，而多数消费者只关心其中一位：「队列里有没有
 * 在跑的」「这一篇的任务」「当前库开没开同步」。把整份快照放进 state，等于每一拍都重渲染
 * 自己连同整棵子树——挂在工作区根上时就是整个工作区（真机 2026-10-15：切录音、开笔记、
 * 知识库页的卡顿）。这里让调用方说清「取哪一位、怎样算没变」：选出来的值与上一次相等，
 * 就把上一次的引用原样交回，React 据此不渲染。
 *
 * 前端零业务（ADR-003）：选择器只做取值与比较，判定仍在 Rust 下发的快照里。
 */

import { useRef, useSyncExternalStore } from "react";

/** 两次选出来的值算不算「没变」。 */
export type SelectionEquality<T> = (previous: T, next: T) => boolean;

/**
 * 订阅一个模块级共享源，只在选出来的值变了时重渲染。
 *
 * `subscribe` 与 `getSnapshot` 必须是稳定引用（模块级函数）：换引用就是重新订阅。
 * `select` 可以每次渲染新建（内联闭包）——参数变了（如换了一篇笔记）当场重算，算出来
 * 与上一次相等时仍交回旧引用。
 */
export function useStoreSelector<S, T>(
  subscribe: (onChange: () => void) => () => void,
  getSnapshot: () => S,
  select: (snapshot: S) => T,
  isEqual: SelectionEquality<T> = Object.is,
): T {
  const memo = useRef<{ snapshot: S; select: (snapshot: S) => T; value: T } | null>(null);
  const getSelection = (): T => {
    const snapshot = getSnapshot();
    const last = memo.current;
    if (last !== null && last.snapshot === snapshot && last.select === select) {
      return last.value;
    }
    const next = select(snapshot);
    const value = last !== null && isEqual(last.value, next) ? last.value : next;
    memo.current = { snapshot, select, value };
    return value;
  };
  return useSyncExternalStore(subscribe, getSelection);
}

/**
 * 结构相等：给「选出来的是一小段 DTO（数组 / 对象）」的选择器用。
 *
 * 广播每一拍都是反序列化出来的新对象，引用比较恒为「变了」；而这几段 DTO 很小（一篇笔记的
 * 几条任务、一个投影），按内容比一次远比让整棵子树重渲染一遍便宜。
 */
export function sameStructure<T>(previous: T, next: T): boolean {
  return JSON.stringify(previous) === JSON.stringify(next);
}
