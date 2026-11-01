/**
 * 「试跑一次」：发一次**真任务**（走同一条主干，ADR-001），于是它通过就意味着真任务也
 * 通过——「测试连接」只探控制面，答不了鉴权、额度、模型分配这些真正会拦住人的东西
 * （ADR-126 第 8 条）。结果里最要紧的是「谁跑的」：它与卡头那句「AI 在哪运行」是同一件
 * 事的两种来源，对不上就是这一页在说谎。
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { useComputeStatus } from "../../hooks/use-compute-status";
import { refreshUsage } from "../usage/use-usage";
import { commands, type SelfCheckResultDto } from "../../ipc/bindings";

type State = { kind: "idle" } | { kind: "running" } | { kind: "done"; result: SelfCheckResultDto };

export function ComputeSelfCheck() {
  const { t } = useTranslation("settings");
  const { t: tCommon } = useTranslation("common");
  const [state, setState] = useState<State>({ kind: "idle" });
  const status = useComputeStatus();

  /**
   * 这条路变了就丢掉上一次的结论——过期的成功结论恰好长得像"这条路没问题"。
   *
   * **身份取「对话这条能力在哪跑、为什么」，不是这台机器的落点**（ADR-126 第 10 条）：
   * 从「服务端优先」切到「本机优先」时机器落点一个字不变，而执行者已经换了。
   */
  const chatRoute = status?.routes.find((route) => route.capability === "chat");
  const routeIdentity = chatRoute ? `${chatRoute.location}|${chatRoute.reason}` : null;
  useEffect(() => {
    setState({ kind: "idle" });
  }, [routeIdentity]);

  const run = useCallback(async () => {
    setState({ kind: "running" });
    const result = await commands.computeSelfCheck();
    // 这一次真的记了一条账，而账是 Rust 写的——下方那张表得被通知（ADR-126 第 9 条）。
    refreshUsage();
    // 命令本身失败与"任务没跑通"是两件事，但对用户是同一句话：没通。
    setState(
      result.status === "ok"
        ? { kind: "done", result: result.data }
        : {
            kind: "done",
            result: {
              ok: false,
              executed_by: null,
              location: null,
              duration_ms: 0,
              error_code: null,
            },
          },
    );
  }, []);

  return (
    // 页脚带：左边一句话、右边一颗动作。没跑过时左边是「这颗按钮干什么」，
    // 跑过之后换成结论——同一个位置回答同一个问题，不多占一行（ADR-127）。
    <footer className="luna-compute-card__footer">
      <p
        className="luna-compute-card__footer-text"
        data-ok={state.kind === "done" ? state.result.ok : undefined}
        role={state.kind === "done" ? "status" : undefined}
      >
        {state.kind !== "done"
          ? t("self_check.hint")
          : state.result.ok
            ? t("self_check.ok", {
                executor: state.result.executed_by ?? t("self_check.unknown_executor"),
                ms: state.result.duration_ms,
              })
            : // 失败翻成人话：走与弹窗、对话同一份 `common:failure.*`，绝不裸显（TDD-15 §4.3）。
              t("self_check.failed", {
                reason: state.result.error_code
                  ? tCommon(`failure.${state.result.error_code}`, {
                      defaultValue: tCommon("failure.generic"),
                    })
                  : tCommon("failure.generic"),
              })}
      </p>
      <button
        type="button"
        className="luna-btn luna-btn--secondary"
        disabled={state.kind === "running"}
        onClick={() => void run()}
      >
        {state.kind === "running" ? t("self_check.running") : t("self_check.action")}
      </button>
    </footer>
  );
}
