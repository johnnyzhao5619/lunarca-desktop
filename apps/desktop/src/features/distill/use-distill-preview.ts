/**
 * 重生成差异预览的控制器（PRD-06-045）。
 *
 * 与批量整理同一模式：入口在对象所在处（徽标 / 右键），预览对话框由唯一常驻宿主承载
 * （`DISTILL_PREVIEW_EVENT` 事件桥），派发方不必知道对话框在哪儿。
 *
 * 预览产物暂存在 Rust（单槽），故本 Hook 只管交互状态——「接受」调 `distill_apply`
 * 直接落盘，不重跑 LLM。
 */

import { useCallback, useEffect, useState } from "react";
import { commands, type DistillPreviewDto } from "../../ipc/bindings";
import { DISTILL_PREVIEW_EVENT, type DistillPreviewRequestDetail } from "../../lib/app-events";

export type DistillPreviewState =
  | { kind: "closed" }
  | { kind: "loading"; notePath: string }
  | { kind: "error"; message: string }
  | { kind: "ready"; notePath: string; preview: DistillPreviewDto; applying: boolean };

export interface DistillPreviewController {
  state: DistillPreviewState;
  close: () => void;
  apply: () => Promise<void>;
}

export function useDistillPreview(options: {
  /** 落盘后打开产出的笔记（由宿主决定在哪个目的地打开）。 */
  onApplied: (path: string) => void;
}): DistillPreviewController {
  const [state, setState] = useState<DistillPreviewState>({ kind: "closed" });
  const { onApplied } = options;

  useEffect(() => {
    const onRequest = (event: Event) => {
      const { notePath, template, fullText } = (
        event as CustomEvent<DistillPreviewRequestDetail>
      ).detail;
      setState({ kind: "loading", notePath });
      // 模板/整理稿开关一路带到 Rust：它们既决定产出结构，也是预览槽的命中判据
      // （换了模板还复用旧产物算差异是谎报）。未指定时传 null = 由 Rust 解析。
      void commands.distillPreview(notePath, template ?? null, fullText ?? null).then((result) => {
        // 只在**仍是本次请求的等待态**时落结果：用户中途关掉对话框（生成可达分钟级，
        // 不能把人锁在遮罩里）后，迟到的结果绝不把它复活——产物已暂存 Rust 单槽，
        // 用户想要时重开预览即刻可得（同槽命中，不重跑）。
        setState((current) =>
          current.kind === "loading" && current.notePath === notePath
            ? result.status === "ok"
              ? { kind: "ready", notePath, preview: result.data, applying: false }
              : { kind: "error", message: result.error }
            : current,
        );
      });
    };
    window.addEventListener(DISTILL_PREVIEW_EVENT, onRequest);
    return () => window.removeEventListener(DISTILL_PREVIEW_EVENT, onRequest);
  }, []);

  const close = useCallback(() => {
    // 落盘请求在途时不给关（distill_apply 是快速本地写，窗口极短）；生成中**可关**——
    // 生成是分钟级的，锁住遮罩才是伤害。关闭不取消后台生成：结果仍会进 Rust 暂存槽。
    setState((current) =>
      current.kind === "ready" && current.applying ? current : { kind: "closed" },
    );
  }, []);

  const apply = useCallback(async () => {
    let target: string | null = null;
    setState((current) => {
      if (current.kind !== "ready") {
        return current;
      }
      target = current.notePath;
      return { ...current, applying: true };
    });
    if (target === null) {
      return;
    }
    const result = await commands.distillApply(target);
    if (result.status === "ok") {
      const path = result.data.note_path;
      setState({ kind: "closed" });
      if (path) {
        onApplied(path);
      }
    } else {
      setState({ kind: "error", message: result.error });
    }
  }, [onApplied]);

  return { state, close, apply };
}
