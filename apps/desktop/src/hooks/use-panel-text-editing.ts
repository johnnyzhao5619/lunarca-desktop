/**
 * 非激活面板里的文本框：编辑期让面板给输入法候选窗让路（TDD-05 §5.1.1）。
 *
 * 面板常驻 `NSStatusWindowLevel`，比系统输入法的候选窗还高——在书签备注框 / 弹窗输入框 /
 * 快捷对话 composer 里打拼音时候选窗被面板盖住（真机 2026-09-06）。层级归 Rust 管，这里只
 * 上报「正在编辑 / 已离开」；非面板窗口（主窗口里的同一组件）由 Rust 判定为 no-op。
 *
 * 卸载即视为离开：书签提示会带着输入框一起超时消失，WKWebView 对被移除元素不保证发 blur。
 *
 * 按下指针也算开始编辑（Windows，ADR-163 ①）：非激活面板不是前台窗口，网页拿不到系统焦点，
 * 输入框的 focus 事件要等面板被激活后才会到——而激活它正是这次上报要 Rust 做的事。指针事件
 * 不受此限，所以在按下那一刻就上报，Rust 激活面板后 focus 随之到达（去重，不会报两次）。
 */
import { useCallback, useEffect, useRef } from "react";
import { commands } from "../ipc/bindings";

export function usePanelTextEditing() {
  const editingRef = useRef(false);

  const report = useCallback((editing: boolean) => {
    if (editingRef.current === editing) {
      return;
    }
    editingRef.current = editing;
    void commands.panelTextEditing(editing);
  }, []);

  useEffect(
    () => () => {
      if (editingRef.current) {
        editingRef.current = false;
        void commands.panelTextEditing(false);
      }
    },
    [],
  );

  return {
    onFocus: () => report(true),
    onPointerDown: () => report(true),
    onBlur: () => report(false),
  };
}
