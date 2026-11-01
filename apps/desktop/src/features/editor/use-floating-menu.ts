import { useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import {
  commands,
  type FloatingMenuAction,
  type FloatingMenuLanding,
  type FloatingMenuMeta,
  type FloatingMenuOptionsDto,
  type FloatingMenuPendingDto,
  type FloatingMenuScope,
  type SearchHitDto,
  type TranslateEvent,
} from "../../ipc/bindings";
import { failureMessage } from "../../i18n/failure";
import { i18n } from "../../i18n/i18n";

export type FloatingMenuStatus = "idle" | "running" | "done" | "failed" | "cancelled" | "error";

export interface FloatingMenuResult {
  status: FloatingMenuStatus;
  output: string;
  message: string | null;
  action: FloatingMenuAction | null;
  /** 输入超长被截断时保留的字符数（PRD-06-025 显式质量边界提示）。 */
  truncatedChars: number | null;
  /** 本轮事实（建议落地 / 执行位置 / 发送字数），流首由 Rust 下发；提交前为 `null`。 */
  meta: FloatingMenuMeta | null;
  /** 分段摘要走到第几段；`null` = 不是分段任务。 */
  chunkProgress: { done: number; total: number } | null;
}

export interface FloatingMenuSearchState {
  status: "idle" | "loading" | "done" | "error";
  query: string;
  hits: SearchHitDto[];
  message: string | null;
}

/** 选区前后段落的原样切片（`ai-session.ts::contextAround`）；取不取由 Rust 定。 */
export interface FloatingMenuContext {
  before: string;
  after: string;
}

export interface FloatingMenuRunOptions {
  scope: FloatingMenuScope;
  instruction?: string | null;
  /** 整篇模式随「总结」传给 Rust：录音笔记 → 会议纪要装配（PRD-04-054）。 */
  noteRel?: string | null;
  context?: FloatingMenuContext | null;
  /** 子菜单选择（翻译目标语言 / 总结风格）；缺省走设置。 */
  options?: FloatingMenuOptionsDto;
}

/**
 * 候选列表（PRD-06-021，Word Copilot 版本导航对标）：重试 / 追问不覆盖上一版，而是追加
 * 一份候选，用户可左右翻；落地取当前候选。新动作开新列表。
 */
export interface FloatingMenuCandidates {
  items: FloatingMenuResult[];
  /** 当前显示的候选下标。 */
  index: number;
}

export const INITIAL_RESULT: FloatingMenuResult = {
  status: "idle",
  output: "",
  message: null,
  action: null,
  truncatedChars: null,
  meta: null,
  chunkProgress: null,
};

const INITIAL_CANDIDATES: FloatingMenuCandidates = { items: [], index: 0 };

const INITIAL_SEARCH: FloatingMenuSearchState = {
  status: "idle",
  query: "",
  hits: [],
  message: null,
};

export function applyFloatingMenuEvent(
  current: FloatingMenuResult,
  event: TranslateEvent,
): FloatingMenuResult {
  switch (event.kind) {
    case "started":
      return {
        ...current,
        status: "running",
        output: "",
        message: null,
      };
    case "token":
      return {
        ...current,
        status: "running",
        output: current.output + event.data,
        message: null,
      };
    case "done":
    case "cache_hit":
      return {
        ...current,
        status: "done",
        output: event.data,
        message: null,
      };
    case "failed":
      return {
        ...current,
        status: "failed",
        // 稳定错误码 → 本地化修复文案（绝不裸显错误码）。
        message: failureMessage(event.data.code),
      };
    case "cancelled":
      return {
        ...current,
        status: "cancelled",
        message: null,
      };
    // 输入超长已截断（PRD-06-025）：随结果保留提示，直到下一轮提交。
    case "truncated":
      return {
        ...current,
        truncatedChars: event.data,
      };
    // 本轮事实（PRD-06-020/021）：建议落地方式、执行位置、实际发送字数。
    case "floating_menu":
      return {
        ...current,
        meta: event.data,
      };
    // 分段摘要的进度（PRD-06-025）：分段之间没有 token 流，不报进度就与卡死无法区分。
    case "floating_chunk":
      return {
        ...current,
        status: "running",
        chunkProgress: { done: event.data.done, total: event.data.total },
      };
    // 表格 AI 跑完了但结果不能放进正文（抽不出表 / 不是一张表）：按失败呈现、不给落地。
    case "floating_rejected":
      return {
        ...current,
        status: "failed",
        message: i18n.t(`editor:floating.rejected.${event.data}`),
      };
    case "meta":
      return current;
    // auto_copied 是弹窗翻译的剪贴板回执（PRD-01-032），悬浮菜单结果区无此动作。
    case "auto_copied":
      return current;
  }
}

/** 当前候选（没有候选时为空闲态）。 */
export function currentCandidate(candidates: FloatingMenuCandidates): FloatingMenuResult {
  return candidates.items[candidates.index] ?? INITIAL_RESULT;
}

/** 追加一份新候选（生成中）并切到它；`fresh` = 新动作，先清空旧候选。 */
export function pushCandidate(
  candidates: FloatingMenuCandidates,
  running: FloatingMenuResult,
  fresh: boolean,
): FloatingMenuCandidates {
  const items = fresh ? [running] : [...candidates.items, running];
  return { items, index: items.length - 1 };
}

/** 把流事件套到指定下标的候选上（其它候选不动）。 */
export function updateCandidate(
  candidates: FloatingMenuCandidates,
  at: number,
  update: (current: FloatingMenuResult) => FloatingMenuResult,
): FloatingMenuCandidates {
  if (!candidates.items[at]) {
    return candidates;
  }
  const items = candidates.items.map((item, i) => (i === at ? update(item) : item));
  return { ...candidates, items };
}

interface FloatingMenuRequest {
  text: string;
  action: FloatingMenuAction;
  options: FloatingMenuRunOptions;
}

export function useFloatingMenu() {
  const [candidates, setCandidates] = useState<FloatingMenuCandidates>(INITIAL_CANDIDATES);
  const [lastRequest, setLastRequest] = useState<FloatingMenuRequest | null>(null);
  const [searchState, setSearchState] = useState<FloatingMenuSearchState>(INITIAL_SEARCH);
  const result = currentCandidate(candidates);

  async function submit(
    text: string,
    action: FloatingMenuAction,
    options: FloatingMenuRunOptions,
    fresh: boolean,
  ) {
    const inputText = text.trim();
    if (!inputText) {
      return;
    }
    const instruction = options.instruction?.trim() ?? null;
    if ((action === "ask" || action === "table_fill_column") && !instruction) {
      return;
    }
    setLastRequest({ text: inputText, action, options });
    setSearchState(INITIAL_SEARCH);
    const running: FloatingMenuResult = {
      status: "running",
      output: "",
      message: null,
      action,
      truncatedChars: null,
      meta: null,
      chunkProgress: null,
    };
    // 这一轮候选的下标在提交那一刻定下来，流事件只改它自己那一份。
    let slot = -1;
    setCandidates((current) => {
      const next = pushCandidate(current, running, fresh);
      slot = next.index;
      return next;
    });
    const channel = new Channel<TranslateEvent>();
    channel.onmessage = (event) => {
      setCandidates((current) =>
        updateCandidate(current, slot, (item) => applyFloatingMenuEvent(item, event)),
      );
    };
    const outcome = await commands.submitFloatingMenu(
      inputText,
      action,
      options.scope,
      instruction,
      options.noteRel ?? null,
      options.context ? { before: options.context.before, after: options.context.after } : null,
      options.options ?? { target_lang: null, summary_style: null, table_column: null },
      channel,
    );
    if (outcome.status === "error") {
      setCandidates((current) =>
        updateCandidate(current, slot, (item) => ({
          ...item,
          status: "error",
          output: "",
          message: outcome.error,
        })),
      );
    }
  }

  /** 新动作：清空旧候选，开一份新的。 */
  async function run(text: string, action: FloatingMenuAction, options: FloatingMenuRunOptions) {
    await submit(text, action, options, true);
  }

  /** 追问（PRD-06-021）：以当前候选的输出为输入，按新指令继续改写，追加为新候选。 */
  async function followUp(instruction: string) {
    if (result.status !== "done" || !result.output.trim() || !lastRequest) {
      return;
    }
    await submit(result.output, "ask", { scope: lastRequest.options.scope, instruction }, false);
  }

  /** 停止当前生成（保留已产 token，后端回 `cancelled` 终态）。 */
  function stop() {
    void commands.floatingMenuStop();
  }

  async function search(text: string) {
    const selectedText = text.trim();
    if (!selectedText) {
      return;
    }
    setCandidates(INITIAL_CANDIDATES);
    setSearchState({
      status: "loading",
      query: selectedText,
      hits: [],
      message: null,
    });
    const outcome = await commands.search({
      text: selectedText,
      note_type: null,
      tag: null,
      path_prefix: null,
      limit: 20,
    });
    if (outcome.status === "ok") {
      setSearchState({
        status: "done",
        query: selectedText,
        hits: outcome.data,
        message: null,
      });
      return;
    }
    setSearchState({
      status: "error",
      query: selectedText,
      hits: [],
      message: outcome.error,
    });
  }

  /** 重新生成：同一请求再跑一次，追加为新候选（失败态则原地重跑，不堆失败候选）。 */
  function retry() {
    if (!lastRequest) {
      return;
    }
    const failed =
      result.status === "failed" || result.status === "error" || result.status === "cancelled";
    if (failed) {
      setCandidates((current) => ({
        items: current.items.filter((_, i) => i !== current.index),
        index: Math.max(0, current.index - 1),
      }));
    }
    void submit(lastRequest.text, lastRequest.action, lastRequest.options, candidates.items.length <= 1 && failed);
  }

  function selectCandidate(index: number) {
    setCandidates((current) => ({
      ...current,
      index: Math.max(0, Math.min(index, current.items.length - 1)),
    }));
  }

  /**
   * 落地回执（PRD-06-021）：这一份结果最终替换 / 插入 / 丢弃了——Rust 记进用量库，
   * 接受率才算得出来。只报动作名与方式，不含正文。
   */
  function land(landing: FloatingMenuLanding) {
    if (result.action) {
      void commands.floatingMenuLanded(result.action, landing);
    }
  }

  /**
   * 注入一份**后台跑完、等着落地**的结果（PRD-06-021）：它是这扇面板的唯一候选，
   * 与前台跑出来的那份走同一套落地按钮与回执——后台不另开一条落地通路。
   */
  function seed(pending: FloatingMenuPendingDto) {
    setCandidates({
      items: [
        {
          status: "done",
          output: pending.output,
          message: null,
          action: pending.action,
          truncatedChars: null,
          meta: pending.meta,
          chunkProgress: null,
        },
      ],
      index: 0,
    });
    setSearchState(INITIAL_SEARCH);
  }

  function reset() {
    setCandidates(INITIAL_CANDIDATES);
    setSearchState(INITIAL_SEARCH);
  }

  /** 最近一次自定义指令（追问也算）；「存为预设」用。 */
  const lastInstruction =
    lastRequest?.action === "ask" ? (lastRequest.options.instruction?.trim() ?? null) : null;

  return {
    result,
    lastInstruction,
    candidateIndex: candidates.index,
    candidateCount: candidates.items.length,
    selectCandidate,
    searchState,
    run,
    followUp,
    stop,
    search,
    retry,
    land,
    seed,
    reset,
  };
}
