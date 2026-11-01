// AI 结果区（PRD-06-021）：流式预览 → 用户拍板落地。
//
// 三种预览形态（Apple 写作工具 / Word Copilot 对标）：
// · 改写 / 生成类：Markdown 渲染的新文，「显示原文」一键切回原选区比对；
// · 校对：原文上标出逐条变更（删 / 增），每条可单独接受或拒绝，落地文本按接受集合拼出；
// · 候选翻页：重试 / 追问不覆盖上一版，左右翻，落地取当前这份。
// 主按钮 = Rust 给的**建议落地方式**，其余落地方式退为次级；复制 / 朗读 / 存为新笔记 /
// 重新生成收进与弹窗翻译共用的结果操作条。接受走 ⌘↩、丢弃走 Esc——刻意不对称。

import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { FloatingMenuLanding, FloatingMenuScope } from "../../ipc/bindings";
import { MarkdownContent } from "../../components/markdown/MarkdownContent";
import { ResultActionBar } from "../popup-translate/ResultActionBar";
import {
  ChevronLeftIcon,
  ChevronRightIcon,
  SendIcon,
  StopCircleIcon,
} from "../../components/ui/Icon";
import { applyChanges, diffChanges, diffSegments, type DiffChange } from "./ai-diff";
import type { FloatingMenuResult } from "./use-floating-menu";
import { isImeKeyEvent } from "../../lib/ime-key-event";

/** 各作用域可选的落地方式（顺序即次级按钮顺序；建议方式会被提到最前）。 */
const LANDINGS: Record<FloatingMenuScope, readonly FloatingMenuLanding[]> = {
  selection: ["replace", "insert_below"],
  cursor: ["insert_cursor"],
  doc: ["insert_top", "insert_end"],
};

/** 主按钮 + 次级按钮：建议方式在前，其余按作用域固定顺序。 */
export function orderedLandings(
  scope: FloatingMenuScope,
  suggested: FloatingMenuLanding | null,
): FloatingMenuLanding[] {
  const options = LANDINGS[scope];
  if (suggested && options.includes(suggested)) {
    return [suggested, ...options.filter((landing) => landing !== suggested)];
  }
  return [...options];
}

interface AiResultPanelProps {
  scope: FloatingMenuScope;
  result: FloatingMenuResult;
  /** 打开面板时的原文（选区 / 光标前文 / 整篇），供「显示原文」与校对差异。 */
  originalText: string;
  candidateIndex: number;
  candidateCount: number;
  onSelectCandidate: (index: number) => void;
  /** 执行位置标记（流首事实到达后）；`null` = 不渲染。 */
  routeLabel: string | null;
  acceptShortcut: string;
  onStop: () => void;
  onRetry: () => void;
  onReject: () => void;
  onCopy: (text: string) => void;
  onSaveNote: (text: string) => void;
  /** 落地：`text` 是最终要写进正文的文本（校对时按接受集合拼出）。 */
  onLand: (landing: FloatingMenuLanding, text: string) => void;
  onFollowUp: (instruction: string) => void;
  /** 把这一轮的自定义指令存为预设（PRD-06-024）；`null` = 这一轮不是自定义指令。 */
  onSavePreset: ((name: string) => void) | null;
  /** 校对某处变更的「为什么」（按需、单次）；返回一句解释。 */
  onExplain: (change: DiffChange) => Promise<string>;
  /** 转入后台（只有整篇作用域给得出）；`null` = 这一轮不能转。 */
  onBackground: (() => void) | null;
}

/** 结果以逐条差异审阅、而不是整段替换的动作（作用域须为选区）。 */
const DIFF_REVIEW_ACTIONS: ReadonlySet<string> = new Set([
  "proofread",
  "script_tighten",
  "script_hook",
  "script_spoken",
  "script_dictation",
]);

export function AiResultPanel({
  scope,
  result,
  originalText,
  candidateIndex,
  candidateCount,
  onSelectCandidate,
  routeLabel,
  acceptShortcut,
  onStop,
  onRetry,
  onReject,
  onCopy,
  onSaveNote,
  onLand,
  onFollowUp,
  onSavePreset,
  onExplain,
  onBackground,
}: AiResultPanelProps) {
  const { t } = useTranslation("editor");
  const outputRef = useRef<HTMLDivElement | null>(null);
  const [draft, setDraft] = useState("");
  /** 「存为预设」的名字输入；`null` = 未展开。 */
  const [presetName, setPresetName] = useState<string | null>(null);
  const [showOriginal, setShowOriginal] = useState(false);
  const running = result.status === "running";
  const done = result.status === "done";
  const canUseResult = done && result.output.trim().length > 0;
  const failed =
    result.status === "failed" || result.status === "error" || result.status === "cancelled";
  // 逐条差异审阅：校对与视频脚本的四个改稿动作（PRD-19-014：改稿以差异呈现，确认才替换）。
  const reviewByDiff = scope === "selection" && result.action !== null && DIFF_REVIEW_ACTIONS.has(result.action);
  const landings = orderedLandings(scope, result.meta?.suggested_landing ?? null);

  // 校对差异：定稿后一次算好；逐条接受集合默认全选。
  const changes = useMemo<DiffChange[]>(
    () => (reviewByDiff && done ? diffChanges(originalText, result.output) : []),
    [reviewByDiff, done, originalText, result.output],
  );
  const [rejected, setRejected] = useState<ReadonlySet<number>>(new Set());
  /** 最近悬停 / 聚焦的那处变更（解释行跟着它）。 */
  const [focusedChange, setFocusedChange] = useState<number | null>(null);
  type Explanation = { status: "loading" } | { status: "done"; text: string } | { status: "failed" };
  const [explanations, setExplanations] = useState<ReadonlyMap<number, Explanation>>(new Map());
  useEffect(() => {
    setRejected(new Set());
    setShowOriginal(false);
    setFocusedChange(null);
    setExplanations(new Map());
  }, [candidateIndex, result.status]);

  function explain(change: DiffChange) {
    setExplanations((current) => new Map(current).set(change.id, { status: "loading" }));
    void onExplain(change).then(
      (text) => {
        setExplanations((current) => new Map(current).set(change.id, { status: "done", text }));
      },
      () => {
        setExplanations((current) => new Map(current).set(change.id, { status: "failed" }));
      },
    );
  }
  const accepted = useMemo(
    () => new Set(changes.filter((change) => !rejected.has(change.id)).map((change) => change.id)),
    [changes, rejected],
  );
  const landingText = reviewByDiff ? applyChanges(originalText, changes, accepted) : result.output;
  const canLand = canUseResult && !showOriginal && (!reviewByDiff || accepted.size > 0);

  // 流式输出跟到底：用户看的是「正在写到哪」，不是开头。
  useEffect(() => {
    const element = outputRef.current;
    if (element && running) {
      element.scrollTop = element.scrollHeight;
    }
  }, [result.output, running]);

  function submitFollowUp() {
    const instruction = draft.trim();
    if (!instruction) {
      return;
    }
    setDraft("");
    onFollowUp(instruction);
  }

  function toggleChange(id: number) {
    setRejected((current) => {
      const next = new Set(current);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  }

  const segments = reviewByDiff && done ? diffSegments(originalText, changes) : [];

  return (
    <section className="luna-floating-menu__result" aria-live="polite">
      <div className="luna-floating-menu__result-bar">
        {routeLabel ? <span className="luna-floating-menu__route">{routeLabel}</span> : null}
        {candidateCount > 1 ? (
          <span className="luna-floating-menu__pager" role="group" aria-label={t("floating.result.candidates")}>
            <button
              type="button"
              className="luna-floating-menu__pager-btn"
              aria-label={t("floating.result.prev")}
              disabled={candidateIndex <= 0}
              onClick={() => onSelectCandidate(candidateIndex - 1)}
            >
              <ChevronLeftIcon size={12} />
            </button>
            <span>
              {t("floating.result.candidate", { index: candidateIndex + 1, count: candidateCount })}
            </span>
            <button
              type="button"
              className="luna-floating-menu__pager-btn"
              aria-label={t("floating.result.next")}
              disabled={candidateIndex >= candidateCount - 1}
              onClick={() => onSelectCandidate(candidateIndex + 1)}
            >
              <ChevronRightIcon size={12} />
            </button>
          </span>
        ) : null}
        {canUseResult && !reviewByDiff ? (
          <button
            type="button"
            className="luna-floating-menu__context-toggle"
            aria-pressed={showOriginal}
            onClick={() => setShowOriginal((current) => !current)}
          >
            {t("floating.result.show_original")}
          </button>
        ) : null}
      </div>
      {result.truncatedChars !== null ? (
        <p className="luna-floating-menu__notice" role="status">
          {t("floating.truncated", { count: result.truncatedChars })}
        </p>
      ) : null}
      {running ? (
        <div className="luna-floating-menu__status">
          <span className="luna-floating-menu__spinner" aria-hidden="true" />
          <span>
            {result.chunkProgress && result.chunkProgress.total > 0
              // 逐行填写这一列（T6）与分段摘要共用同一条进度事件，文案按动作分开：这里是第几行，不是第几段摘要。
              ? t(result.action === "table_fill_column" ? "floating.table.fill_progress" : "floating.preflight.progress", {
                  done: Math.min(result.chunkProgress.done + 1, result.chunkProgress.total),
                  total: result.chunkProgress.total,
                })
              : t("floating.status.running")}
          </span>
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-floating-menu__stop"
            onClick={onStop}
          >
            <StopCircleIcon size={14} />
            {t("floating.action.stop")}
          </button>
          {onBackground ? (
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-floating-menu__stop"
              title={t("floating.action.background_title")}
              onClick={onBackground}
            >
              {t("floating.action.background")}
            </button>
          ) : null}
        </div>
      ) : null}
      {failed ? (
        <p className="luna-floating-menu__status" role="status">
          {t(`floating.status.${result.status}`)}
          {result.message ? `: ${result.message}` : ""}
        </p>
      ) : null}
      {reviewByDiff && done ? (
        <div className="luna-floating-menu__output luna-floating-menu__diff">
          {changes.length === 0 ? (
            <p className="luna-floating-menu__diff-empty">{t("floating.diff.none")}</p>
          ) : (
            <>
              <p className="luna-floating-menu__diff-summary">
                {t("floating.diff.count", { count: changes.length, accepted: accepted.size })}
                <button
                  type="button"
                  className="luna-floating-menu__diff-all"
                  onClick={() => setRejected(new Set())}
                >
                  {t("floating.diff.accept_all")}
                </button>
                <button
                  type="button"
                  className="luna-floating-menu__diff-all"
                  onClick={() => setRejected(new Set(changes.map((change) => change.id)))}
                >
                  {t("floating.diff.reject_all")}
                </button>
              </p>
              <p className="luna-floating-menu__diff-text">
                {segments.map((segment, i) =>
                  segment.kind === "text" ? (
                    <span key={i}>{segment.text}</span>
                  ) : (
                    <button
                      key={i}
                      type="button"
                      className="luna-floating-menu__diff-change"
                      aria-pressed={!rejected.has(segment.change.id)}
                      title={t("floating.diff.toggle_title")}
                      onMouseEnter={() => setFocusedChange(segment.change.id)}
                      onFocus={() => setFocusedChange(segment.change.id)}
                      onClick={() => toggleChange(segment.change.id)}
                    >
                      {segment.change.original ? <del>{segment.change.original}</del> : null}
                      {segment.change.replacement ? <ins>{segment.change.replacement}</ins> : null}
                    </button>
                  ),
                )}
              </p>
              {focusedChange !== null && changes[focusedChange] ? (
                <p className="luna-floating-menu__diff-explain">
                  <span className="luna-floating-menu__diff-explain-what">
                    {changes[focusedChange].original || "∅"} → {changes[focusedChange].replacement || "∅"}
                  </span>
                  {(() => {
                    const state = explanations.get(focusedChange);
                    if (!state) {
                      return (
                        <button
                          type="button"
                          className="luna-floating-menu__diff-all"
                          onClick={() => explain(changes[focusedChange])}
                        >
                          {t("floating.diff.explain")}
                        </button>
                      );
                    }
                    if (state.status === "loading") {
                      return <span>{t("floating.diff.explaining")}</span>;
                    }
                    if (state.status === "failed") {
                      return <span>{t("floating.diff.explain_failed")}</span>;
                    }
                    return <span>{state.text}</span>;
                  })()}
                </p>
              ) : null}
            </>
          )}
        </div>
      ) : showOriginal ? (
        <pre className="luna-floating-menu__output luna-floating-menu__output--original">
          {originalText}
        </pre>
      ) : result.output ? (
        <div ref={outputRef} className="luna-floating-menu__output">
          <MarkdownContent content={result.output} streaming={running} />
        </div>
      ) : null}
      {canUseResult ? (
        <>
          <div className="luna-floating-menu__landing">
            {landings.map((landing, index) => (
              <button
                key={landing}
                type="button"
                className={`luna-btn luna-btn--${index === 0 ? "primary" : "secondary"}`}
                disabled={!canLand}
                title={
                  index === 0
                    ? t("toolbar.labelShortcut", {
                        label: t(`floating.action.${landing}`),
                        shortcut: acceptShortcut,
                      })
                    : undefined
                }
                onClick={() => {
                  onLand(landing, landingText);
                }}
              >
                {t(`floating.action.${landing}`)}
              </button>
            ))}
            <button type="button" className="luna-btn luna-btn--secondary" onClick={onReject}>
              {t("floating.action.reject")}
            </button>
          </div>
          <ResultActionBar
            canUseResult={canUseResult}
            canSaveNote={canUseResult}
            showReplace={false}
            copyVariant="secondary"
            copyLabel={t("floating.action.copy")}
            speakLabel={t("floating.action.speak")}
            onCopy={() => onCopy(landingText)}
            onRegenerate={onRetry}
            onReplace={() => {
              onLand("replace", landingText);
            }}
            onSaveNote={() => onSaveNote(landingText)}
            speakText={landingText}
            speakLang={null}
            speakScope={`editor-ai:${scope}`}
          />
          {onSavePreset ? (
            presetName === null ? (
              <button
                type="button"
                className="luna-floating-menu__preset-save"
                onClick={() => setPresetName("")}
              >
                {t("floating.presets.save")}
              </button>
            ) : (
              <div className="luna-floating-menu__ask">
                <input
                  type="text"
                  className="luna-input"
                  value={presetName}
                  placeholder={t("floating.presets.name_placeholder")}
                  aria-label={t("floating.presets.name_placeholder")}
                  onChange={(event) => setPresetName(event.target.value)}
                  onKeyDown={(event) => {
                    if (isImeKeyEvent(event)) return;
                    if (event.key === "Enter" && presetName.trim()) {
                      event.preventDefault();
                      onSavePreset(presetName.trim());
                      setPresetName(null);
                    }
                    if (event.key === "Escape") {
                      event.stopPropagation();
                      setPresetName(null);
                    }
                  }}
                />
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary luna-floating-menu__send"
                  disabled={!presetName.trim()}
                  onClick={() => {
                    onSavePreset(presetName.trim());
                    setPresetName(null);
                  }}
                >
                  {t("floating.presets.confirm")}
                </button>
              </div>
            )
          ) : null}
          <div className="luna-floating-menu__ask">
            <input
              type="text"
              className="luna-input"
              value={draft}
              placeholder={t("floating.followup_placeholder")}
              aria-label={t("floating.ask_label")}
              onChange={(event) => {
                setDraft(event.target.value);
              }}
              onKeyDown={(event) => {
                if (isImeKeyEvent(event)) return;
                if (event.key === "Enter") {
                  event.preventDefault();
                  submitFollowUp();
                }
              }}
            />
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-floating-menu__send"
              disabled={!draft.trim()}
              aria-label={t("floating.action.send")}
              onClick={submitFollowUp}
            >
              <SendIcon size={14} />
            </button>
          </div>
        </>
      ) : null}
      {failed ? (
        <div className="luna-floating-menu__landing">
          <button type="button" className="luna-btn luna-btn--secondary" onClick={onRetry}>
            {t("floating.action.retry")}
          </button>
          <button type="button" className="luna-btn luna-btn--secondary" onClick={onReject}>
            {t("floating.action.reject")}
          </button>
        </div>
      ) : null}
    </section>
  );
}
