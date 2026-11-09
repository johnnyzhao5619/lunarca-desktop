import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  events,
  type FetchInstallPlanDto,
  type FetchToolStatusDto,
  type InstallToolchainDto,
} from "../ipc/bindings";
import { formatBytes } from "@lunarca/editor-core/format-bytes";
import { vaultErrorText } from "../lib/vault-error";

/**
 * 受管工具安装进度复用模型下载事件；这是两件受管工具在事件里的 id
 * （Rust `fetch_tool::TOOL_ID` / `JS_RUNTIME_TOOL_ID`）→ 展示名。
 *
 * **必须分开**：合并安装是"一次点击、两个下载"，共用一个 id 会让进度条在第二件开始时
 * 莫名归零，而界面说不出此刻在下的是哪一件。
 */
const TOOL_EVENT_NAMES: Record<string, string> = {
  "yt-dlp": "yt-dlp",
  deno: "Deno",
};

/** 计划在点击与执行之间变了（Rust `fetch_tool::PLAN_CHANGED`）。 */
export const PLAN_CHANGED = "FETCH_TOOL_PLAN_CHANGED";

/** 安装进度：`{ 工具展示名, 已下载/总量 }`；`progress === ""` = 还在查上游版本。 */
export interface ToolInstallProgress {
  name: string;
  progress: string;
}

export interface FetchToolchain {
  /** 工具可用性（`null` = 尚未探测完成或探测失败）。 */
  status: FetchToolStatusDto | null;
  /** 点「安装」之前的计划（`null` = 尚未取回）。 */
  plan: FetchInstallPlanDto | null;
  /** 正在探测工具（首次探测要跑一次 `--version`，PyInstaller 工具约 8 秒）。 */
  checking: boolean;
  /** 安装进行中的进度（`null` = 没在装）。 */
  installing: ToolInstallProgress | null;
  /** 最近一次安装的结果（区分"两件都装上"与"yt-dlp 装上、运行时失败"）。 */
  outcome: InstallToolchainDto | null;
  /** 最近一次合并安装失败的错误码（`FETCH_TOOL_PLAN_CHANGED` = 计划已刷新，需要再确认）。 */
  installError: string | null;
  /**
   * JS 运行时那一半没装上的原因：合并安装的 `js_runtime_error`，或单独安装运行时失败。
   * yt-dlp 已经可用，故它不是整次失败，但必须在运行时那一处说出口。
   */
  runtimeError: string | null;
  /** 重新探测（`force` 无视 Rust 侧缓存）；计划随之重取。 */
  refresh: (force?: boolean) => Promise<void>;
  /** 按当前看到的计划安装；成功（含运行时那一半失败）返回 `true`。 */
  install: () => Promise<boolean>;
  /** 单独安装受管 JS 运行时（yt-dlp 已就绪、运行时缺失时）。 */
  installJsRuntime: () => Promise<boolean>;
  /** 清掉上一次的安装结论（关闭提示、改去做别的事时）。 */
  dismissOutcome: () => void;
}

/**
 * 抓取工具链的状态 / 计划 / 安装——「从链接导入」对话框与「外部工具」面板的**唯一**实现
 * （PRD-05-007 2026-10-08 修订）。
 *
 * 此前两处各写一份安装流程：面板会说出"运行时那一半没装上"，对话框却静默吞掉它；
 * 两处都用一段固定说明代替计划（"会一并装上 Deno"——本机已有 node 时根本不装）。
 * 现在计划由 Rust 给（`fetch_install_plan`），执行时把看过的组件 id 原样交回核对：
 * 环境在两次点击之间变了，Rust 回 `FETCH_TOOL_PLAN_CHANGED`，这里重取计划等用户再确认，
 * 而不是按旧计划装或悄悄多装一件。
 */
export function useFetchToolchain(): FetchToolchain {
  // 体积串的数值本地化按界面语言（TDD-11 §1.4b）。
  const locale = useTranslation().i18n.language;
  const [status, setStatus] = useState<FetchToolStatusDto | null>(null);
  const [plan, setPlan] = useState<FetchInstallPlanDto | null>(null);
  const [checking, setChecking] = useState(false);
  const [installing, setInstalling] = useState<ToolInstallProgress | null>(null);
  const [outcome, setOutcome] = useState<InstallToolchainDto | null>(null);
  const [installError, setInstallError] = useState<string | null>(null);
  const [runtimeInstallError, setRuntimeInstallError] = useState<string | null>(null);

  const refreshPlan = useCallback(async () => {
    setPlan(await commands.fetchInstallPlan());
  }, []);

  const refresh = useCallback(
    async (force = false) => {
      setChecking(true);
      try {
        const result = await commands.fetchToolStatus(force);
        setStatus(result.status === "ok" ? result.data : null);
        await refreshPlan();
      } finally {
        setChecking(false);
      }
    },
    [refreshPlan],
  );

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // 安装进度：订阅模型下载事件里 id 为受管工具的那些帧（Rust 侧已按 250ms 节流）。
  useEffect(() => {
    const unlisten = events.modelDownloadProgress.listen((event) => {
      const { model_id, downloaded_bytes, total_bytes, done } = event.payload;
      const name = TOOL_EVENT_NAMES[model_id];
      if (name === undefined) {
        return;
      }
      if (done) {
        setInstalling(null);
        return;
      }
      // 总量未知时只报已下载量，不编造分母。
      setInstalling({
        name,
        progress:
          total_bytes !== null && total_bytes > 0
            ? `${formatBytes(downloaded_bytes, locale)} / ${formatBytes(total_bytes, locale)}`
            : formatBytes(downloaded_bytes, locale),
      });
    });
    return () => {
      void unlisten.then((off) => off());
    };
    // 进度串在回调里拼，闭包捕获订阅那一刻的 locale：切语言即重订阅。
  }, [locale]);

  const install = useCallback(async () => {
    if (plan === null) {
      return false;
    }
    setInstallError(null);
    setRuntimeInstallError(null);
    setOutcome(null);
    const first = plan.components[0]?.tool === "js_runtime" ? "deno" : "yt-dlp";
    setInstalling({ name: TOOL_EVENT_NAMES[first] ?? first, progress: "" });
    try {
      const result = await commands.installFetchTool(plan.components.map((part) => part.tool));
      if (result.status === "error") {
        const code = vaultErrorText(result.error);
        setInstallError(code);
        if (code.startsWith(PLAN_CHANGED)) {
          await refreshPlan();
        }
        return false;
      }
      setOutcome(result.data);
      // 装完立刻重探（force：装的就是新文件）；Rust 侧安装末尾已预热，通常命中缓存。
      await refresh(true);
      return true;
    } finally {
      setInstalling(null);
    }
  }, [plan, refresh, refreshPlan]);

  const installJsRuntime = useCallback(async () => {
    setInstallError(null);
    setRuntimeInstallError(null);
    setOutcome(null);
    setInstalling({ name: TOOL_EVENT_NAMES.deno as string, progress: "" });
    try {
      const result = await commands.installFetchJsRuntime();
      if (result.status === "error") {
        setRuntimeInstallError(vaultErrorText(result.error));
        return false;
      }
      await refresh(true);
      return true;
    } finally {
      setInstalling(null);
    }
  }, [refresh]);

  const dismissOutcome = useCallback(() => {
    setOutcome(null);
    setInstallError(null);
    setRuntimeInstallError(null);
  }, []);

  return {
    status,
    plan,
    checking,
    installing,
    outcome,
    installError,
    runtimeError: outcome?.js_runtime_error ?? runtimeInstallError,
    refresh,
    install,
    installJsRuntime,
    dismissOutcome,
  };
}
