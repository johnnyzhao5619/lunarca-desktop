/**
 * 知识体检报告（PRD-16-030/031/032）：建议卡列表 + 深度体检的确认卡与进度。
 *
 * 三条纪律直接对应 `PRD-16-032`：
 * ① **只出建议、绝不自动修**——卡上没有任何"帮我改"的按钮；「归入主题页…」通向的是
 *    批次 2 就有的确认卡（差异预览 + 版本快照），修不修、怎么修都由用户在那儿定；
 * ② **忽略零成本**——✕ 即对这条永久安静（落库随库走）；积压卡没有 ✕，它会自己消失；
 * ③ **深度体检显式触发**——先给数字（多少条声明、多少页、近重复能不能跑）再给开始按钮，
 *    与全库重扫 / 语义建库同一套六要素骨架。
 *
 * 卡片文案全部按**种类**查 i18n（种类串来自 Rust），前端不拼人读句子；「是不是问题」
 * 的判断一律在 Rust（ADR-003）。
 */

import { useCallback, useEffect, useState } from "react";
import { IconButton } from "../../components/ui/IconButton";
import { useTranslation } from "react-i18next";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { inlineMarkdownToText } from "@lunarca/editor-core/markdown";
import {
  commands,
  events,
  type CheckupFindingDto,
  type CheckupPreflightDto,
  type CheckupProgress,
  type CheckupReportDto,
} from "../../ipc/bindings";
import { CloseIcon, SparklesIcon, StethoscopeIcon } from "@lunarca/editor-core/Icon";
import {
  CHECKUP_EVENT,
  type CheckupDetail,
  requestEditorReveal,
  requestShowDrafts,
} from "../../lib/app-events";
import { requestGraphRescan } from "./graph-scope-menu";
import { requestDistillBatch } from "../distill/distill-actions";
import { noteStem } from "../../lib/note-path";
import { formatRelativeTime } from "../../lib/format-relative-time";
import { fixAction, primaryAction, primaryActionKey, type CheckupAction } from "./checkup-actions";
import { vaultErrorText } from "../../lib/vault-error";
import { IndexingNotice } from "../../components/ui/IndexingNotice";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { openNote } from "../../lib/navigate";

function runAction(action: CheckupAction, close: () => void): void {
  switch (action.kind) {
    case "note":
      close();
      openNote(action.path);
      if (action.line !== null) {
        // 暂存 + 广播：目标笔记此刻多半还没打开，纯事件会派发进真空（遗留清理轮的同一修法）。
        // `headingIndex: null` = 定位目标是普通行而非标题（阅读模式此时只打开笔记、
        // 不假装滚到某处——与搜索命中同一口径）。
        requestEditorReveal({ path: action.path, line: action.line, headingIndex: null, focusEditor: true });
      }
      break;
    case "distill-batch":
      close();
      requestDistillBatch(null);
      break;
    case "rescan":
      // 「还没有标签」→ 全库补齐确认卡（同一个常驻宿主模式；那张卡会列出被停用的文件夹）。
      close();
      requestGraphRescan(null);
      break;
    case "reveal":
      // 读不了的文件夹（V1b）：在访达里亮出来，改权限是用户在系统里做的事；面板留着，
      // 改完回来看一眼这张卡还在不在（下一次扫描后才会消失）。
      void commands.revealVaultPath(action.path);
      break;
    case "show-drafts":
      // 待沉淀积压（PRD-06-055）→ 掀开文件树的「隐藏记录」，并把最旧的那一篇打开
      // （页头的「记录」徽标 = 一键沉淀，「生成知识笔记」徽标 = AI 辅助沉淀）。
      // 只带路、不代劳：这张卡不会替用户沉淀任何一篇。
      close();
      requestShowDrafts();
      if (action.path !== null) {
        openNote(action.path);
      }
      break;
  }
}

export function CheckupDialog() {
  const { t, i18n } = useTranslation("workspace");
  const [open, setOpen] = useState(false);
  const [report, setReport] = useState<CheckupReportDto | null>(null);
  const [preflight, setPreflight] = useState<CheckupPreflightDto | null>(null);
  /** 随报告一起取的预检：只用来回答「此刻能不能深检」（见 `refresh` 里的说明）。 */
  const [readiness, setReadiness] = useState<CheckupPreflightDto | null>(null);
  const [progress, setProgress] = useState<CheckupProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  /** 「针对本页体检」的视角（PRD-07-059a / TDD-07 §4.1d）：只过滤**既有**发现，
      不改变体检命令的作用域——报告本身仍是全库的那一份。 */
  const [focusPath, setFocusPath] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    // 预检**随报告一起取**（2026-07-30 的 D9）：深度体检能不能跑，是一个由数字回答的
    // 问题（有多少条声明、语义就不就绪）。此前要等用户点了「深度体检…」才发现前置不满足，
    // 而那句 `CHECKUP_NOTHING_TO_DO` 还被渲染成**红色错误**——它不是出事，是"此刻没什么
    // 可深检的"。判定仍在 Rust（前端只读它给的数字，不解析错误串）。
    const [next, running, pre] = await Promise.all([
      commands.checkupReport(),
      commands.checkupDeepStatus(),
      commands.checkupPreflight(),
    ]);
    setReport(next.status === "ok" ? next.data : null);
    setProgress(running.status === "ok" ? running.data : null);
    setReadiness(pre.status === "ok" ? pre.data : null);
    setNow(Date.now());
    if (next.status !== "ok") {
      setError(vaultErrorText(next.error));
    }
  }, []);

  useEffect(() => {
    const onRequest = (event: Event) => {
      setError(null);
      setPreflight(null);
      setFocusPath((event as CustomEvent<CheckupDetail | undefined>).detail?.focusPath ?? null);
      setOpen(true);
      void refresh();
    };
    window.addEventListener(CHECKUP_EVENT, onRequest);
    return () => window.removeEventListener(CHECKUP_EVENT, onRequest);
  }, [refresh]);

  useEffect(() => {
    // 进度是流、快照是状态：重开面板靠 status 恢复，运行期靠事件推进（同语义建库）。
    const unlisten = events.checkupProgress.listen((event) => {
      setProgress(event.payload.finished ? null : event.payload);
      if (event.payload.finished) {
        void refresh();
      }
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [refresh]);

  // 开着的时候库变了（含开库那一轮扫描做完，V1b）就重取：报告是索引上的几条 SQL。
  useVaultReload(() => void refresh(), { enabled: open });

  const close = useCallback(() => setOpen(false), []);

  const ignore = useCallback(
    (id: string) => {
      void commands.checkupIgnore(id).then((result) => {
        if (result.status !== "ok") {
          setError(vaultErrorText(result.error));
          return;
        }
        void refresh();
      });
    },
    [refresh],
  );

  // 模态行为（Esc / 焦点圈定 / 焦点归还）统一走共享 hook——本卡此前**完全没有 Esc**。
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose: close, enabled: open });

  if (!open) {
    return null;
  }

  const allFindings = report?.findings ?? [];
  // 「本页视角」= 只留涉及这一页的发现。判定仍是 Rust 那一份（`paths` 由它给出），
  // 这里只是一个过滤器——过滤不掉的那部分照样存在，只是此刻不在看。
  const findings = focusPath
    ? allFindings.filter((finding) => finding.paths.includes(focusPath))
    : allFindings;
  const running = progress !== null || report?.deep_running === true;

  return (
    <div className="luna-overlay" role="presentation" onClick={close}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-checkup"
        role="dialog"
        aria-modal="true"
        aria-label={t("checkup.title")}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-checkup__header">
          <h3>
            <StethoscopeIcon size={15} />
            {t("checkup.title")}
          </h3>
          {/* 对话框头部关闭钮统一为幽灵图标钮：它此前是一颗描边的次要按钮，与旁边
              真正的次要动作同重，读起来像"这里有两个可选动作"。 */}
          <IconButton label={t("checkup.close")} icon={<CloseIcon size={14} />} onClick={close} />
        </header>
        {/* 开库那一轮扫描期间报告数的是半截索引（V1b）；扫描完由 useVaultReload 重取。 */}
        <IndexingNotice />

        {/* 深度体检：进行中给进度与取消；否则给「上次跑于何时」+ 确认卡 */}
        <section className="luna-checkup__deep">
          {running && progress ? (
            <div className="luna-checkup__progress" role="status" aria-live="polite">
              <progress
                className="luna-progress"
                value={progress.done}
                max={Math.max(progress.total, 1)}
              />
              <span>
                {t("checkup.deepProgress", { done: progress.done, total: progress.total })}
                {progress.found > 0 ? t("checkup.deepFound", { count: progress.found }) : null}
                {progress.failed > 0 ? t("checkup.deepFailed", { count: progress.failed }) : null}
              </span>
              <button
                type="button"
                className="luna-btn luna-btn--secondary"
                onClick={() => void commands.checkupDeepCancel()}
              >
                {t("checkup.deepCancel")}
              </button>
            </div>
          ) : preflight ? (
            <div className="luna-checkup__confirm">
              <p className="luna-checkup__confirm-title">
                {t("checkup.confirmTitle", {
                  claims: preflight.claims,
                  topics: preflight.topics,
                })}
              </p>
              <p className="luna-checkup__hint">
                {preflight.semantic_ready
                  ? t("checkup.confirmWithSemantic")
                  : t("checkup.confirmNoSemantic")}
              </p>
              <div className="luna-dialog__actions">
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary"
                  onClick={() => setPreflight(null)}
                >
                  {t("checkup.cancel")}
                </button>
                <button
                  type="button"
                  className="luna-btn luna-btn--primary"
                  onClick={() => {
                    setError(null);
                    void commands.checkupDeepStart().then((result) => {
                      setPreflight(null);
                      if (result.status !== "ok") {
                        setError(vaultErrorText(result.error));
                      } else {
                        void refresh();
                      }
                    });
                  }}
                >
                  {t("checkup.deepStart")}
                </button>
              </div>
            </div>
          ) : (
            <div className="luna-checkup__deep-idle">
              <span className="luna-checkup__hint">
                {report?.deep_ran_at_ms
                  ? t("checkup.lastDeep", {
                      when: formatRelativeTime(report.deep_ran_at_ms, now, i18n.language),
                    })
                  : t("checkup.neverDeep")}
              </span>
              {/* 前置不满足 → 按**说明**呈现（灰字，说清缺哪一半），不占用错误色、
                  也不给一个点了必然失败的按钮。 */}
              {readiness && readiness.claims === 0 && !readiness.semantic_ready ? (
                <span className="luna-checkup__hint">{t("checkup.deepNotReady")}</span>
              ) : (
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary"
                  // 快照说「有一次在跑」而进度帧还没到（刚开始的那一瞬）：按钮此时可点的话，
                  // 用户得到的只会是一句 CHECKUP_BUSY——如实置灰比让他撞一次墙诚实。
                  disabled={running}
                  onClick={() => {
                    setError(null);
                    void commands.checkupPreflight().then((result) => {
                      if (result.status !== "ok") {
                        setError(vaultErrorText(result.error));
                        return;
                      }
                      setPreflight(result.data);
                    });
                  }}
                >
                  <SparklesIcon size={14} />
                  {t("checkup.deep")}
                </button>
              )}
            </div>
          )}
        </section>

        {/* 「本页视角」必须**说出来并且可退出**：不说的话，一份被过滤过的报告看起来
            就是全库报告，而"这一页干净"与"全库干净"是两句不同的话。 */}
        {focusPath !== null && (
          <p className="luna-checkup__hint">
            {t("checkup.focus", { title: noteStem(focusPath) })}{" "}
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-btn--sm"
              onClick={() => setFocusPath(null)}
            >
              {t("checkup.focusClear")}
            </button>
          </p>
        )}

        {findings.length === 0 ? (
          <div className="luna-checkup__empty">
            <p>{t("checkup.clean")}</p>
            <p className="luna-checkup__hint">{t("checkup.cleanHint")}</p>
          </div>
        ) : (
          <ul className="luna-checkup__list">
            {findings.map((finding) => (
              <CheckupCard
                key={finding.id}
                finding={finding}
                onAct={(action) => runAction(action, close)}
                onIgnore={() => ignore(finding.id)}
              />
            ))}
          </ul>
        )}

        {report && report.truncated > 0 ? (
          <p className="luna-checkup__hint">
            {t("checkup.truncated", { count: report.truncated })}
          </p>
        ) : null}
        {error ? (
          <p className="luna-checkup__error" role="alert">
            {error}
          </p>
        ) : null}
      </div>
    </div>
  );
}

function CheckupCard({
  finding,
  onAct,
  onIgnore,
}: {
  finding: CheckupFindingDto;
  onAct: (action: CheckupAction) => void;
  onIgnore: () => void;
}) {
  const { t } = useTranslation("workspace");
  const primary = primaryAction(finding);
  const fix = fixAction(finding);
  return (
    <li className="luna-checkup__card" data-kind={finding.kind}>
      <div className="luna-checkup__card-main">
        <p className="luna-checkup__card-title">{t(`checkup.card.${finding.kind}.title`)}</p>
        <p className="luna-checkup__hint">
          {t(`checkup.card.${finding.kind}.desc`, {
            count: finding.count,
            subject: finding.subject,
          })}
        </p>
        {/* 「怎么办」是这张卡的落脚点：体检**刻意不自动改**任何东西（PRD-16-032），
            那就更该把下一步说清——只报告"这里不对"而不说该干什么，等于把判断和劳动
            都推回给用户。逐类给（每类的正解不同），不给一句通用的"请检查"。 */}
        <p className="luna-checkup__todo">{t(`checkup.card.${finding.kind}.todo`)}</p>
        {/* 模型给的理由是**内容**，逐字呈现；它旁边那句"这是本地模型的判断"由文案负责。 */}
        {finding.reason ? <p className="luna-checkup__reason">{finding.reason}</p> : null}
        {finding.subject && finding.kind !== "broken-link" ? (
          // 摘录是从笔记正文原样取的一行，常带链接语法——压成可读文本再摆出来。
          <p className="luna-checkup__quote">{inlineMarkdownToText(finding.subject)}</p>
        ) : null}
        {finding.paths.length > 0 ? (
          <p className="luna-checkup__chips">
            {finding.paths.map((path) => (
              <span key={path} className="luna-checkup__chip">
                {noteStem(path)}
              </span>
            ))}
          </p>
        ) : null}
      </div>
      <div className="luna-checkup__card-actions">
        {primary ? (
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            onClick={() => onAct(primary)}
          >
            {t(primaryActionKey(finding))}
          </button>
        ) : null}
        {fix ? (
          <button type="button" className="luna-btn luna-btn--primary" onClick={() => onAct(fix)}>
            {t("checkup.action.fix")}
          </button>
        ) : null}
        {finding.ignorable ? (
          <button
            type="button"
            className="luna-checkup__ignore"
            title={t("checkup.action.ignore")}
            aria-label={t("checkup.action.ignore")}
            onClick={onIgnore}
          >
            <CloseIcon size={12} />
          </button>
        ) : null}
      </div>
    </li>
  );
}
