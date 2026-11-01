/**
 * 同步回放面板（TDD-07 §3.1 / PRD-04 §3.4）。
 *
 * - 媒体来源 = 笔记 frontmatter `luna.media[]`（经 loadSyncPlayback 聚合，前端零路径推导）
 * - 多分片经媒体服务虚拟拼接呈现为单一连续时间轴（PRD-04 §5）
 * - role="listbox" cue 列表，每项 role="option"（a11y）
 * - timeupdate + rAF 驱动高亮（不新增 IPC）
 * - follow-mode：用户滚动 → manual；点击「回到当前」→ following
 * - 进度条拖动/点击 seek + 倍速（0.75–2x，对标 Teams）+ 书签跳转（PRD-04-005）
 * - 双语三态（译文为空时禁用 translation/both 切换）
 * - 非颜色-only 高亮（active item 同时有边框+背景，a11y）
 * - 二次翻译（PRD-04-034）：对既有转写段按目标语言批量转译并权威回写，点亮译文/双语；
 *   改目标语言可重译（幂等清旧译）。
 * - 字幕质量校验（PRD-05-033）：每条按 CPS/行长/时长标注超标（只提示不阻断——观众读不完
 *   的字幕等于没有字幕，但用户有权导出一条超标的），底栏给整篇汇总供交付前自检。
 * - 布局契约（PRD-04-058 真机反馈）：本视图**自己撑满主区并在内部滚动**——播放控制（时间/
 *   进度条/播放·倍速·显示模式）常驻顶部、处理动作常驻底部，中间只有转写列表滚动。控制条随
 *   正文滚走等于「读到第 300 句就没法暂停」；更要命的是分屏态下画面容器会被拉到与整篇转写
 *   等高，`object-fit: contain` 把画面居中到几千像素之下 = 有声无画面。
 * - 编辑交互（PRD-04-031）：
 *   · **双击单条** = 就地编辑该句（连续校对的轻量入口），含**删除本条**（两步确认）；
 *   · **文本编辑模式** = 进入**整篇校对编辑器**（逐行编辑全文、批量原子保存，类笔记文档体验）。
 * - 视频笔记（导入 mp4/mov/m4v，PRD-04-058）：主体区挂**视频舞台** [VideoStage]——画面与
 *   转写并排分屏（默认）/ 剧场 / 悬浮小窗 / 仅音频四选，转写与译文作为字幕叠在画面上。
 * - 说话人手动校正（PRD-04-056，Otter/Descript 对标）：徽标点开指派菜单（目标 × 范围
 *   「仅此条/及后续/全部」），校对模式下逐条可「在光标处拆分」（一条里两个人说话）与
 *   「与上一条合并」（拆分的逆操作）；名册条给出配色图例与全局改名入口。
 * - 导出（PRD-04 §3.4）：SRT / VTT / ASS / LRC / 带时间戳文本 / 纯文本，内容跟随当前双语显示模式；
 *   保存对话框与写盘全在 Rust（ADR-003 前端零业务）。
 */

import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { useTranslation } from "react-i18next";
import { requestExportOptions } from "../../lib/app-events";
import {
  TRANSCRIPT_EXPORT_FORMATS,
  currentExportOptions,
  exportOptionsCustomized,
  useExportOptions,
} from "../transcribe/export-options";
import {
  commands,
  events,
  type CueTextEditDto,
  type ProcTaskDto,
  type SegmentEditDto,
  type SpeakerScopeDto,
  type TranscriptExportFormat,
} from "../../ipc/bindings";
import { TRANSLATE_TARGET_SETTING, useSettingOptions } from "../settings/language-options";
import { ProcessingProgress } from "../processing/ProcessingProgress";
import { isTaskActive } from "../processing/processing-labels";
import { useNoteTasks } from "../processing/use-processing-queue";
import { taskFailureText } from "../../lib/task-failure";
import { QueryBox } from "../../components/ui/QueryBox";
import { IconButton } from "../../components/ui/IconButton";
import {
  BookmarkIcon,
  ChevronDownIcon,
  ChevronUpIcon,
  CopyIcon,
  DownloadIcon,
  FilmIcon,
  LinkIcon,
  MergeIcon,
  PauseIcon,
  PencilIcon,
  PlayIcon,
  ScissorsIcon,
  TrashIcon,
  UserIcon,
} from "../../components/ui/Icon";
import { PLAYBACK_RATES } from "./use-playback";
import { usePlaybackHost } from "./playback-host";
import { findCueMatches, splitByMatch, stepMatch } from "./transcript-find";
import { VideoStage, VideoStageControls } from "./VideoStage";
import { SelectBox } from "../../components/ui/SelectBox";
import { cssVariables } from "../../lib/css-variables";
import { SpeakerMenu } from "./SpeakerMenu";
import { useConfirm } from "../../components/ConfirmHost";
import { clearToStart } from "../../hooks/use-task-start-gate";
import { useContextMenu } from "../../components/ContextMenuHost";
import { buildMenu, separator } from "../../lib/context-menu";
import { glossaryTermFromDocumentSelection } from "../settings/glossary-term";
import { requestGlossaryAdd } from "../settings/GlossaryQuickAdd";
import { SpeakerRoster } from "./SpeakerRoster";
import { speakerDisplayName, speakerSlot } from "./speaker-view";
import { DistillBadge } from "../distill/DistillBadge";
import { formatClock } from "../../lib/format-duration";
import type { CuePoint } from "../recording/recording-cue";
import type { BilingualMode, PlaybackRate } from "./use-playback";
import { vaultErrorText } from "../../lib/vault-error";
import { openNote } from "../../lib/navigate";
import { TranscriptKnowledgeNotice } from "./TranscriptKnowledgeNotice";

// ─── Props ────────────────────────────────────────────────────────────────────

export interface SyncPlaybackViewProps {
  noteRel: string;
}

/** 整篇校对编辑器：按内容估算 textarea 行数（文档流观感，避免定高截断）。 */
function draftRows(text: string): number {
  const lines = text
    .split("\n")
    .reduce((count, line) => count + Math.max(1, Math.ceil(line.length / 46)), 0);
  return Math.min(8, Math.max(2, lines));
}

/** 说话人指派菜单尺寸预估（clamp 到视口用；真实高度随名册长度微差，只影响贴边翻转）。 */
const SPEAKER_MENU_ESTIMATED = { width: 240, height: 300 };
/** 走带 ±跳转步长（PRD-04 §3.4；与主流播放器一致的 10 秒）。 */
const SKIP_MS = 10_000;

// ─── 组件 ─────────────────────────────────────────────────────────────────────

/**
 * 录音只在云端时（官方同步按需下载，ADR-156），装载前 Rust 先把它下到本机：加载期间把下载进度
 * 说出来（百分比），而不是一句一直不变的「加载中」。
 */
function useCloudDownloadPercent(loading: boolean): number | null {
  const [percent, setPercent] = useState<number | null>(null);
  useEffect(() => {
    if (!loading) {
      setPercent(null);
      return;
    }
    const unlisten = events.syncDownloadProgress.listen(({ payload }) => {
      const done = payload.done ?? 0;
      const total = payload.total ?? 0;
      setPercent(total > 0 && done < total ? Math.floor((done * 100) / total) : null);
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [loading]);
  return percent;
}

export function SyncPlaybackView({ noteRel }: SyncPlaybackViewProps) {
  const { t } = useTranslation("playback");
  const { t: tCommon } = useTranslation("common");
  const { t: tTranscribe } = useTranslation("transcribe");
  const { t: tSettings } = useTranslation("settings");
  const contextMenu = useContextMenu();
  const ask = useConfirm();
  // 播放状态**不在本视图里**（ADR-118）：媒体元素与 `usePlayback` 归常驻宿主，本视图
  // 是它的一个投影。于是切到别的空间只卸掉这层投影，声音继续（PRD-04-064）。
  // 视频呈现偏好同样取自宿主（设置仍是唯一事实源），两边不各订一次 settings://changed。
  const { playbackState, actions, videoPrefs, open, registerStage } = usePlaybackHost();
  // 本篇在统一处理队列中的任务（入队序，PRD-04-055）：驱动内联进度/排队/按钮态。
  // 自己按篇订：别的任务推进时这一篇的切片不变，回放视图不重渲染；也不再经工作区根转手。
  const tasks = useNoteTasks(noteRel);

  // 挂载 / 换篇时把这一篇交给宿主装载；同一篇重复交付是空操作，不打断正在听的那段。
  useEffect(() => {
    open(noteRel);
  }, [noteRel, open]);
  // 说话人显示名（PRD-04-053/054）：改名映射（笔记 frontmatter）优先，否则默认轨标签。
  // 解析规则在纯函数 `speakerDisplayName`（可 headless 测试），此处只注入 i18n 文案。
  const speakerLabel = (speaker: string): string =>
    speakerDisplayName(speaker, playbackState.speakerNames, {
      self: tCommon("track.self"),
      remote: tCommon("track.remote"),
      remoteN: (n) => tCommon("track.remoteN", { n }),
      speakerN: (n) => tCommon("track.speakerN", { n }),
    });
  const {
    cues,
    bookmarks,
    activeCueIndex,
    playing,
    positionMs,
    durationMs,
    playbackRate,
    follow,
    error,
    loading,
    bilingualMode,
    speakers,
    nextSpeakerId,
    mediaKind,
    hasMedia,
  } = playbackState;
  const downloadPercent = useCloudDownloadPercent(loading);
  const isVideo = mediaKind === "video";

  // 整篇字幕质量汇总（PRD-05-033）：判定在 Rust（阈值与导出重切分同源），此处只数个数。
  const flaggedCount = useMemo(() => cues.filter((cue) => cue.issues.length > 0).length, [cues]);
  /** 逐条质检徽标默认收起（PRD-05-033）：它服务导出前的校对，不该常驻在每一行的阅读视线里。 */
  const [showQuality, setShowQuality] = useState(false);
  // 视图内查找（PRD-04-028 补记）：在已加载的整篇 cue 里定位"那句话"；命中只高亮不过滤。
  const [findQuery, setFindQuery] = useState("");
  const [findIndex, setFindIndex] = useState(-1);
  const findMatches = useMemo(
    () => findCueMatches(cues, findQuery, bilingualMode),
    [cues, findQuery, bilingualMode],
  );
  const currentMatchCue = findIndex >= 0 ? (findMatches[findIndex] ?? -1) : -1;
  useEffect(() => {
    // 查询或结果集一变就回到第一处；没有命中时不指向任何行。
    setFindIndex(findMatches.length > 0 ? 0 : -1);
  }, [findMatches]);
  useEffect(() => {
    if (currentMatchCue < 0) return;
    const row = listRef.current?.querySelector<HTMLElement>(`#cue-${currentMatchCue}`);
    row?.scrollIntoView({ block: "center" });
    // 跳到命中处 = 用户接管了滚动：跟随回放的自动滚动让位（与手动滚动同一口径）。
    actions.onUserScroll();
  }, [currentMatchCue, actions]);
  function stepFind(delta: 1 | -1) {
    setFindIndex((current) => stepMatch(current, findMatches.length, delta));
  }
  function renderFindable(text: string): ReactNode {
    if (!findQuery.trim()) return text;
    return splitByMatch(text, findQuery).map((part, i) =>
      part.hit ? <mark key={i}>{part.text}</mark> : <span key={i}>{part.text}</span>,
    );
  }
  /** 页头标题 = 文件名去扩展名（与侧栏行同一来源，PRD-07-065：页头答"这是哪一篇"）。 */
  const noteTitle = (noteRel.split("/").pop() ?? noteRel).replace(/\.md$/u, "");

  const listRef = useRef<HTMLOListElement | null>(null);
  const activeCueRef = useRef<HTMLLIElement | null>(null);
  const editInputRef = useRef<HTMLTextAreaElement | null>(null);
  /** 整篇校对每行 textarea（拆分需读取 `selectionStart` = UTF-16 光标位置）。 */
  const draftRefs = useRef(new Map<number, HTMLTextAreaElement>());

  // 单条编辑（双击）：就地编辑一句（PRD-04-031 轻量入口）。
  const [editingCueIndex, setEditingCueIndex] = useState<number | null>(null);
  const [editingText, setEditingText] = useState("");
  const [editingSaving, setEditingSaving] = useState(false);
  const [editingError, setEditingError] = useState<string | null>(null);
  const [editStatus, setEditStatus] = useState<string | null>(null);
  /** 待确认删除的 cue 下标（PRD-04-031 两步确认；`null` = 无）。 */
  const [pendingDelete, setPendingDelete] = useState<number | null>(null);

  // 整篇校对编辑器（PRD-04-031）：逐行编辑全文 + 批量原子保存。
  const [fullEditMode, setFullEditMode] = useState(false);
  const [drafts, setDrafts] = useState<Record<number, string>>({});
  const [savingAll, setSavingAll] = useState(false);

  // 二次翻译（PRD-04-034）目标语言：可选、改语言可重译。候选语言与实时转译同源
  // （取自设置规格，Rust 注册表唯一事实源），"smart" = 智能双向不属语言清单故单列。
  const [translateTarget, setTranslateTarget] = useState<string>("smart");
  const translateTargets = useSettingOptions(TRANSLATE_TARGET_SETTING);
  // 入队命令的即时错误（如模型未装 / 尚无转写）；处理中失败经队列面板反馈。
  const [enqueueError, setEnqueueError] = useState<string | null>(null);
  /** 「取回媒体」的入队态与即时错误（PRD-05-006 仅存链接的回补路径）。 */
  const [refetching, setRefetching] = useState(false);
  const [refetchError, setRefetchError] = useState<string | null>(null);

  /** 取回媒体（PRD-05-006）：形态随动作走——省磁盘的人多半只要音频，要校对画面的给视频。 */
  async function refetchMedia(media: "audio" | "video") {
    setRefetching(true);
    setRefetchError(null);
    try {
      const result = await commands.refetchNoteMedia(noteRel, media);
      if (result.status === "error") {
        setRefetchError(vaultErrorText(result.error));
      }
    } finally {
      setRefetching(false);
    }
  }

  // ── 说话人（PRD-04-054 改名 / PRD-04-056 手动校正）─────────────────────────
  // 改名：名册条与指派菜单共用**同一处**就地输入（永远出现在名册行，位置可预期）。
  const [renamingSpeaker, setRenamingSpeaker] = useState<{
    speaker: string;
    value: string;
  } | null>(null);
  const renameInputRef = useRef<HTMLInputElement | null>(null);
  // 指派菜单：锚定被点击的徽标（fixed 坐标已按视口 clamp）。
  const [speakerMenu, setSpeakerMenu] = useState<{
    index: number;
    x: number;
    y: number;
  } | null>(null);
  /** 结构性改动（指派/拆分/合并）在途：禁用重复触发，避免与重载竞速。 */
  const [speakerBusy, setSpeakerBusy] = useState(false);
  const renamingId = renamingSpeaker?.speaker ?? null;

  useEffect(() => {
    if (renamingId === null) return;
    renameInputRef.current?.focus();
    renameInputRef.current?.select();
  }, [renamingId]);

  function startSpeakerRename(speaker: string) {
    setRenamingSpeaker({
      speaker,
      value: playbackState.speakerNames[speaker] ?? "",
    });
    setEditStatus(null);
  }

  async function saveSpeakerRename() {
    if (!renamingSpeaker) return;
    const { speaker, value } = renamingSpeaker;
    setRenamingSpeaker(null);
    const error = await actions.renameSpeaker(speaker, value);
    setEditStatus(error ? t("speaker.failed", { error }) : t("speaker.saved"));
  }

  function openSpeakerMenu(index: number, trigger: HTMLElement) {
    const rect = trigger.getBoundingClientRect();
    setSpeakerMenu({
      index,
      x: Math.max(0, Math.min(rect.left, window.innerWidth - SPEAKER_MENU_ESTIMATED.width)),
      y: Math.max(0, Math.min(rect.bottom + 4, window.innerHeight - SPEAKER_MENU_ESTIMATED.height)),
    });
    setEditStatus(null);
  }

  /**
   * 结构性段校正（指派 / 拆分 / 合并 / 删除）的统一入口：先落定未保存的校对文本（拆分要按
   * **已落盘**文本的光标位置定位，草稿未存会错位），再发命令，成功后由 Hook 位置无损重载。
   */
  async function runSegmentEdit(edit: SegmentEditDto, successKey: string): Promise<boolean> {
    if (speakerBusy) return false;
    if (fullEditMode && dirtyCount > 0 && !(await saveFullEdit())) return false;
    setSpeakerBusy(true);
    setEditStatus(null);
    const error = await actions.editSegment(edit);
    setSpeakerBusy(false);
    setEditStatus(error ? t("speaker.editFailed", { error }) : t(successKey));
    return error === null;
  }

  async function assignSpeaker(index: number, speaker: string, scope: SpeakerScopeDto) {
    const cue = cues[index];
    if (!cue) return;
    const isNew = speaker === nextSpeakerId;
    const ok = await runSegmentEdit(
      {
        op: "assign",
        data: { start_ms: cue.start_ms, end_ms: cue.end_ms, speaker, scope },
      },
      `speaker.assigned.${scope}`,
    );
    // 新建的说话人先只有一个编号（「对方4」），紧接着给起名字才是完整动作——直接进改名。
    if (ok && isNew) startSpeakerRename(speaker);
  }

  /**
   * 在光标处拆分（校对模式）：光标位置取该行 textarea 的 `selectionStart`（UTF-16 口径）。
   *
   * 右片**沿用原说话人**而不是自动新建：拆分的下一步通常是把后半句指给一位**已存在**的
   * 说话人（自动细分已经认出他，只是这条没切开），自动新建只会往名册里堆无用编号。
   * 拆完提示用户为第二条指派，一次点击即可完成。
   */
  function splitCueAtCaret(index: number) {
    const cue = cues[index];
    const textarea = draftRefs.current.get(index);
    if (!cue || !textarea) return;
    void runSegmentEdit(
      {
        op: "split",
        data: {
          start_ms: cue.start_ms,
          end_ms: cue.end_ms,
          char_offset: textarea.selectionStart,
          speaker: null,
        },
      },
      "speaker.split",
    );
  }

  function mergeCueWithPrevious(index: number): void {
    const cue = cues[index];
    if (!cue) return;
    void runSegmentEdit(
      { op: "merge", data: { start_ms: cue.start_ms, end_ms: cue.end_ms } },
      "speaker.merged",
    );
  }

  /**
   * 删除整条字幕（PRD-04-031）：片头广告、纯口水、静音段被凭空识别出的文本——这些不是
   * 「改文本」能解决的，把文本清空只会留下一条空白字幕（导出后仍占一个序号）。
   *
   * 删除不可撤销且改变段数，故走**两步确认**（[pendingDelete] 记住待确认的那一条）；
   * 成功后单条编辑器必须退出——结构一变，按下标持有的编辑上下文即刻失效。
   */
  async function deleteCue(index: number): Promise<void> {
    const cue = cues[index];
    if (!cue) return;
    setPendingDelete(null);
    const ok = await runSegmentEdit(
      { op: "delete", data: { start_ms: cue.start_ms, end_ms: cue.end_ms } },
      "edit.deleted",
    );
    if (ok) cancelCueEdit();
  }

  /**
   * 字幕行右键（PRD-07-059）：把这一行**所有**已有能力集中给一遍——跳转 / 编辑 /
   * 复制 / 指派说话人 / 与上一条合并 / 删除。
   *
   * 这些动作此前分散在"单击跳转、双击编辑、徽标点开指派、进编辑态才看得到拆分合并删除"
   * 四种手势里，除了双击提示外没有任何可发现性；右键是它们唯一的共同入口。
   */
  function openCueMenu(event: ReactMouseEvent<HTMLElement>, index: number, cue: CuePoint) {
    const row = event.currentTarget;
    const busy = speakerBusy || savingAll || editingSaving;
    // 取**右键当下**的文档选区：字幕文本在选区白名单内（TDD-15 §3.6b），
    // 用户划中一个被认错的词再右键，正是这条功能的主路径。
    const glossaryTerm = glossaryTermFromDocumentSelection();
    contextMenu.open(
      event,
      buildMenu([
        {
          kind: "action",
          id: "seek",
          label: t("aria.seekTo", { time: formatClock(cue.start_ms) }),
          onSelect: () => actions.seekToCue(index),
        },
        !fullEditMode && {
          kind: "action",
          id: "edit",
          label: t("edit.label", { time: formatClock(cue.start_ms) }),
          icon: <PencilIcon size={13} />,
          onSelect: () => startCueEdit(index),
        },
        {
          kind: "action",
          id: "copy",
          label: tCommon("menu.copyText"),
          icon: <CopyIcon size={13} />,
          onSelect: () => {
            // 复制所见：双语模式下连译文一起，与导出内容口径一致。
            const text =
              bilingualMode !== "source" && cue.translation
                ? bilingualMode === "translation"
                  ? cue.translation
                  : `${cue.text}\n${cue.translation}`
                : cue.text;
            void commands.popupCopy(text);
          },
        },
        // 就地加词（PRD-07-061）：**转写字幕是最需要它的地方**——用户正是在这里
        // 看见人名/术语被认错的。判据与编辑器同一份；选区不像词条时该项不出现。
        glossaryTerm !== null && {
          kind: "action",
          id: "addToGlossary",
          label: tSettings("glossary.quickAdd.menu", { term: glossaryTerm }),
          onSelect: () => requestGlossaryAdd(glossaryTerm),
        },
        separator("structure"),
        {
          kind: "action",
          id: "speaker",
          label: t("speaker.menuTitle"),
          icon: <UserIcon size={13} />,
          disabled: busy,
          onSelect: () => openSpeakerMenu(index, row),
        },
        {
          kind: "action",
          id: "merge",
          label: t("speaker.merge"),
          icon: <MergeIcon size={13} />,
          disabled: index === 0 || busy,
          onSelect: () => mergeCueWithPrevious(index),
        },
        {
          kind: "action",
          id: "delete",
          label: t("edit.delete"),
          icon: <TrashIcon size={13} />,
          danger: true,
          disabled: busy,
          // 从菜单进的删除走**确认卡**，从行内进的走就地两步（下方 `renderDeleteButton`）。
          // 这不是两种语言而是一条可以说清的分工：就地两步只用于**已进入编辑态的行**——
          // 校对一篇文稿要删掉几十条，每条弹一次模态没法用；而右键可以发生在任何一行上，
          // 那一行多半没有删除按钮可供进入确认态，设了状态也没有任何东西会亮起来。
          // （版本回滚的面板内两步确认是同一条分工的另一个实例。）
          onSelect: () => {
            void ask({
              title: t("edit.deleteTitle"),
              description: t("edit.deleteDesc"),
              confirmLabel: t("edit.delete"),
            }).then((ok) => {
              if (ok) {
                void deleteCue(index);
              }
            });
          },
        },
      ]),
    );
  }

  /** 删除按钮（单条编辑器 / 整篇校对行内共用）：首次点击进入确认态，再次点击才真删。 */
  function renderDeleteButton(index: number) {
    const confirming = pendingDelete === index;
    return (
      <button
        type="button"
        className="luna-btn luna-btn--danger-subtle luna-btn--sm luna-playback__inline-btn--danger"
        data-confirming={confirming}
        disabled={speakerBusy || savingAll || editingSaving}
        title={t(confirming ? "edit.deleteConfirmHint" : "edit.deleteHint")}
        // 保住整篇校对里 textarea 的光标（同拆分/合并；否则相邻按钮会抢走焦点）。
        onMouseDown={(event) => event.preventDefault()}
        onClick={(event) => {
          event.stopPropagation();
          if (confirming) void deleteCue(index);
          else setPendingDelete(index);
        }}
      >
        <TrashIcon size={12} />
        {t(confirming ? "edit.deleteConfirm" : "edit.delete")}
      </button>
    );
  }

  /** 说话人徽标（cue 行内 / 校对行内共用）：点击打开指派菜单。 */
  function renderSpeakerBadge(index: number, cue: CuePoint) {
    const label = cue.speaker ? speakerLabel(cue.speaker) : t("speaker.unassigned");
    return (
      <button
        type="button"
        className="luna-subtitle__track luna-subtitle__track--editable"
        data-speaker-slot={speakerSlot(cue.speaker, speakers)}
        data-unassigned={cue.speaker === null}
        aria-haspopup="menu"
        aria-expanded={speakerMenu?.index === index}
        title={t("speaker.assignHint")}
        disabled={speakerBusy}
        onClick={(event) => {
          event.stopPropagation();
          openSpeakerMenu(index, event.currentTarget);
        }}
        onDoubleClick={(event) => event.stopPropagation()}
      >
        {label}
      </button>
    );
  }

  // ── 导出（PRD-04 §3.4）：菜单选格式，内容跟随当前双语显示模式 ──────────────
  const [exporting, setExporting] = useState(false);
  const exportOptions = useExportOptions();

  async function runExport(format: TranscriptExportFormat) {
    if (exporting) return;
    setExporting(true);
    setEditStatus(null);
    const content =
      bilingualMode === "both"
        ? "bilingual"
        : bilingualMode === "translation"
          ? "translation"
          : "source";
    const result = await commands.exportTranscript(noteRel, format, content, currentExportOptions());
    setExporting(false);
    if (result.status === "error") {
      setEditStatus(t("export.failed", { error: result.error }));
      return;
    }
    // null = 用户取消保存对话框（无需提示）。
    if (result.data !== null) {
      setEditStatus(t("export.done", { path: result.data }));
    }
  }

  // ── 处理动作 = 入统一处理队列（PRD-04-032/055）：命令仅入队，进度/记录由队列面板 + 内联
  //    进度条（`tasks` 派生）呈现。单 worker 顺序处理天然保证「先转写后翻译」；per-kind 去重
  //    防重复入队（同一笔记同类型未终态即禁用其触发）。──────────────────────────────────
  const activeKindTask = (kind: string): ProcTaskDto | undefined =>
    tasks.find((task) => task.kind === kind && isTaskActive(task));
  const runningTask = tasks.find((task) => task.status.state === "running");
  const anyActive = tasks.some(
    (task) => task.status.state === "queued" || task.status.state === "running",
  );
  const lastFailed =
    !anyActive && tasks.length > 0 && tasks[tasks.length - 1].status.state === "failed"
      ? tasks[tasks.length - 1]
      : null;
  const retranscribeTask = activeKindTask("retranscribe");
  const translateTask = activeKindTask("translate");
  const diarizeTask = activeKindTask("diarize");
  /** 处理按钮文案：未入队=空闲文案；运行中=运行文案；排队中=「排队中」。 */
  const actionLabel = (task: ProcTaskDto | undefined, running: string, idle: string): string =>
    task ? (task.status.state === "running" ? running : t("queued")) : idle;

  // 队列命令入队失败（模型未装 / 尚无转写）即时提示；成功入队清错。
  /**
   * 二次转写：服务端转写时先问一句够不够（PRD-13-040 ②）——不够当场说清、不入队；够或在本机跑则
   * 照常。两个入口（操作栏与知识提示）共用这一处。
   */
  async function retranscribe() {
    if (await clearToStart({ kind: "retranscribe", note_rel: noteRel }, ask)) {
      await enqueue(commands.enqueueRetranscribe(noteRel));
    }
  }

  async function enqueue(run: Promise<{ status: "ok" | "error"; error?: string }>) {
    setEnqueueError(null);
    setEditStatus(null);
    const result = await run;
    if (result.status === "error") setEnqueueError(result.error ?? null);
  }

  // 二次翻译完成后自动切「双语」（重载后 hasTranslation 即为真，立刻可见成果）。
  const seenDoneTranslateRef = useRef<Set<string>>(new Set());
  useEffect(() => {
    for (const task of tasks) {
      if (
        task.kind === "translate" &&
        task.status.state === "done" &&
        !seenDoneTranslateRef.current.has(task.id)
      ) {
        seenDoneTranslateRef.current.add(task.id);
        actions.setBilingualMode("both");
      }
    }
  }, [tasks, actions]);

  // ── 单条编辑（双击）──────────────────────────────────────────────────────
  function startCueEdit(index: number) {
    const cue = cues[index];
    if (!cue) return;
    actions.seekToCue(index);
    setEditingCueIndex(index);
    setEditingText(cue.text);
    setEditingError(null);
    setEditStatus(null);
  }

  function cancelCueEdit() {
    setEditingCueIndex(null);
    setEditingText("");
    setEditingError(null);
  }

  async function saveCueEdit(cue: CuePoint) {
    const nextText = editingText.trim();
    if (!nextText) {
      setEditingError(t("edit.empty"));
      return;
    }
    setEditingSaving(true);
    setEditingError(null);
    const result = await commands.updatePlaybackCue(
      noteRel,
      cue.start_ms,
      cue.end_ms,
      nextText,
      cue.translation,
    );
    setEditingSaving(false);
    if (result.status === "ok") {
      setEditingCueIndex(null);
      setEditingText("");
      setEditStatus(t("edit.saved"));
      actions.retry();
      return;
    }
    setEditingError(result.error || t("edit.failed"));
  }

  // ── 整篇校对编辑器（PRD-04-031）──────────────────────────────────────────
  // 进入编辑或 cues 重载（保存/翻译后）时以最新 cues 初始化草稿，逐行文本可编辑。
  useEffect(() => {
    if (!fullEditMode) return;
    setDrafts(Object.fromEntries(cues.map((cue, index) => [index, cue.text])));
  }, [fullEditMode, cues]);

  const dirtyIndices = useMemo(
    () =>
      cues.reduce<number[]>((acc, cue, index) => {
        const draft = drafts[index];
        if (draft !== undefined && draft.trim() !== cue.text) acc.push(index);
        return acc;
      }, []),
    [cues, drafts],
  );
  const dirtyCount = dirtyIndices.length;

  function enterFullEdit() {
    cancelCueEdit();
    setRenamingSpeaker(null);
    setFullEditMode(true);
    setEditStatus(null);
  }

  function discardFullEdit() {
    setFullEditMode(false);
    setDrafts({});
  }

  /** 保存整篇改动（原子批量写回）；返回是否成功。 */
  async function saveFullEdit(): Promise<boolean> {
    if (dirtyCount === 0) return true;
    const edits: CueTextEditDto[] = dirtyIndices.map((index) => ({
      start_ms: cues[index].start_ms,
      end_ms: cues[index].end_ms,
      text: (drafts[index] ?? "").trim(),
    }));
    if (edits.some((edit) => !edit.text)) {
      setEditStatus(t("fullEdit.empty"));
      return false;
    }
    setSavingAll(true);
    setEditStatus(null);
    const result = await commands.updatePlaybackTranscript(noteRel, edits);
    setSavingAll(false);
    if (result.status === "error") {
      setEditStatus(t("fullEdit.failed", { error: result.error }));
      return false;
    }
    setEditStatus(t("fullEdit.saved"));
    actions.retry(); // 静默写不发 vault://changed，主动重载让 cues/草稿归位。
    return true;
  }

  /** 完成编辑：有改动先保存（成功才退出），无改动直接退出。 */
  async function finishFullEdit() {
    if (dirtyCount > 0) {
      const ok = await saveFullEdit();
      if (!ok) return;
    }
    setFullEditMode(false);
    setDrafts({});
  }

  // ── follow-mode: 自动居中活动行 ──────────────────────────────────────────

  useEffect(() => {
    if (follow.mode !== "following") return;
    const item = activeCueRef.current;
    if (item) {
      item.scrollIntoView({ block: "nearest", behavior: "smooth" });
    }
  }, [activeCueIndex, follow.mode]);

  useEffect(() => {
    if (editingCueIndex === null) return;
    editInputRef.current?.focus();
    editInputRef.current?.select();
  }, [editingCueIndex]);

  // cues 一变（删除/拆分/合并/重载）下标即换所指，待确认的删除必须作废——否则确认态会
  // 落到另一条身上。
  useEffect(() => {
    setPendingDelete(null);
  }, [cues]);

  // ── 滚动监听：用户手动滚动 → manual ──────────────────────────────────────

  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    let scrollTimer: number | null = null;

    function onScroll() {
      // 节流：50ms 内不重复触发
      if (scrollTimer !== null) return;
      scrollTimer = window.setTimeout(() => {
        scrollTimer = null;
        actions.onUserScroll();
      }, 50);
    }

    list.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      list.removeEventListener("scroll", onScroll);
      if (scrollTimer !== null) window.clearTimeout(scrollTimer);
    };
  }, [actions]);

  // ── 键盘处理 ─────────────────────────────────────────────────────────────

  function onKeyDown(event: React.KeyboardEvent<HTMLOListElement>) {
    const target = event.target as HTMLElement;
    if (target.closest("textarea, input, button")) return;
    // 整篇编辑态下列表键盘 seek 让位于文本编辑。
    if (fullEditMode) return;
    switch (event.key) {
      case " ":
        event.preventDefault();
        actions.togglePlay();
        break;
      case "ArrowDown":
        event.preventDefault();
        if (activeCueIndex < cues.length - 1) {
          actions.seekToCue(activeCueIndex + 1);
        }
        break;
      case "ArrowUp":
        event.preventDefault();
        if (activeCueIndex > 0) {
          actions.seekToCue(activeCueIndex - 1);
        }
        break;
      case "Enter":
        event.preventDefault();
        if (activeCueIndex >= 0) actions.seekToCue(activeCueIndex);
        break;
      case "Escape":
        event.preventDefault();
        // Esc 先撤销「待确认删除」（离危险动作最近的那一层），无待确认才回到当前行。
        if (pendingDelete !== null) setPendingDelete(null);
        else actions.returnToCurrent();
        break;
    }
  }

  function onEditorKeyDown(event: React.KeyboardEvent<HTMLTextAreaElement>, cue: CuePoint) {
    if (event.key === "Escape") {
      event.preventDefault();
      cancelCueEdit();
      return;
    }
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      void saveCueEdit(cue);
    }
  }

  // ── 双语模式：是否有任何译文（实时转译/二次转译/二次翻译产物，PRD-04-025）──────
  const hasTranslation = cues.some((cue) => Boolean(cue.translation));

  const bilingualOptions: {
    mode: BilingualMode;
    label: string;
    disabled: boolean;
  }[] = [
    { mode: "source", label: t("bilingual.source"), disabled: false },
    {
      mode: "translation",
      label: t("bilingual.translation"),
      disabled: !hasTranslation,
    },
    { mode: "both", label: t("bilingual.both"), disabled: !hasTranslation },
  ];

  // 进度条上限：聚合包时长 → 媒体元素解码时长兜底 → 1（防 0 除）
  const scrubberMax = Math.max(durationMs, 1);

  // 视频布局（PRD-04-058）：剧场态让画面占满内容区并暂隐转写列表（画面自带字幕，信息不丢）；
  // **整篇校对时强制退回分屏**——校对必须看得见每一行文本，画面不能把它盖掉。
  const videoLayout =
    isVideo && fullEditMode && videoPrefs.layout === "theater" ? "split" : videoPrefs.layout;
  const theaterMode = isVideo && videoLayout === "theater";
  /** 转写空态：加载完成、无错误、无 cue，且不在剧场态（剧场态整个列表让位于画面）。 */
  const transcriptEmpty = !loading && !error && cues.length === 0 && !theaterMode;

  // ── 渲染 ─────────────────────────────────────────────────────────────────

  return (
    <section className="luna-paper luna-playback" aria-label={t("title")} data-video={isVideo}>
      {/* 播放控制区：**常驻不随转写滚动**——读到第 300 句时同样要能暂停/调速/切显示模式 */}
      <div className="luna-playback__transport">
        <header className="luna-main-header luna-playback__header">
          {/* 页头答的是"这是哪一篇、多长、几个人说、从哪来"（PRD-07-065）——不再写死
              「同步回放」四个字：这一屏没有别的标题，而用户要认的是自己的那场会。 */}
          <div className="luna-main-header__title luna-playback__identity">
            <h1>{noteTitle}</h1>
            <p className="luna-playback__meta" aria-label={isVideo ? t("video.title") : t("title")}>
              <span>{formatClock(durationMs)}</span>
              {speakers.length > 0 && (
                <span>{t("meta.speakers", { count: speakers.length })}</span>
              )}
              {/* 来源（PRD-05-006）：从链接导入的转写稿带一枚可点的原页地址——三个月后回看，
                  用户要能一键回到原视频核对上下文。经系统浏览器打开（不在应用内加载外部页面）。 */}
              {playbackState.sourceUrl !== null && (
                <button
                  type="button"
                  className="luna-playback__source"
                  title={playbackState.sourceUrl}
                  onClick={() => {
                    void commands.openExternalUrl(playbackState.sourceUrl as string);
                  }}
                >
                  <LinkIcon size={12} />
                  <span>{tTranscribe("sourceLink")}</span>
                </button>
              )}
            </p>
          </div>
          {/* 沉淀入口（PRD-06-044）。「来源层」身份不再挂徽标——它的三条待遇改在上下文面板
              「信息」页签说明（PRD-06-048②）。 */}
          <DistillBadge
            notePath={noteRel}
            onOpenNote={(path) => {
              openNote(path);
            }}
          />
          <span className="luna-playback__position" aria-live="off" aria-label={t("aria.position")}>
            {formatClock(positionMs)}
            <span className="luna-playback__duration" aria-hidden="true">
              {" / "}
              {formatClock(durationMs)}
            </span>
          </span>
        </header>

        {/* 仅存链接的转写（PRD-05-006）没有本地媒体：**不给一排点了没反应的播放控件**，
            而是如实说明并给两个真正可用的动作——回原页，或把媒体取回来点亮回放。 */}
        {!hasMedia && (
          <div className="luna-playback__no-media">
            <p>{tTranscribe("linkOnly.playbackHint")}</p>
            <div className="luna-playback__no-media-actions">
              <button
                type="button"
                className="luna-btn luna-btn--primary"
                disabled={refetching}
                onClick={() => void refetchMedia("audio")}
              >
                <DownloadIcon size={14} />
                <span>
                  {refetching
                    ? tTranscribe("linkOnly.fetching")
                    : tTranscribe("linkOnly.fetchAudio")}
                </span>
              </button>
              {/* 要对着画面校对字幕的人得能取回视频——否则只剩"重新导入一次"这条路，
                  那会另建一篇笔记，把已校对的文稿丢在旧的那篇里。 */}
              <button
                type="button"
                className="luna-btn luna-btn--secondary"
                disabled={refetching}
                onClick={() => void refetchMedia("video")}
              >
                <FilmIcon size={14} />
                <span>{tTranscribe("linkOnly.fetchVideo")}</span>
              </button>
              {playbackState.sourceUrl !== null && (
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary"
                  onClick={() => {
                    void commands.openExternalUrl(playbackState.sourceUrl as string);
                  }}
                >
                  <LinkIcon size={14} />
                  <span>{tTranscribe("linkOnly.openSource")}</span>
                </button>
              )}
            </div>
            {refetchError !== null && (
              <p className="luna-playback__no-media-error" role="alert">
                {taskFailureText(refetchError, tTranscribe)}
              </p>
            )}
          </div>
        )}

        {/* 这一篇能不能作主题依据（PRD-16-058/065）：只有实时稿时说实话，已完成转写而这台设备
            还没回答过时在这里问一次。 */}
        <TranscriptKnowledgeNotice
          noteRel={noteRel}
          onRetranscribe={
            hasMedia ? () => void retranscribe() : null
          }
          retranscribing={Boolean(retranscribeTask)}
        />

        {/* 进度条（点击/拖动 seek，PRD-04-024） */}
        <input
          type="range"
          className="luna-range"
          min={0}
          max={scrubberMax}
          step={100}
          value={Math.min(positionMs, scrubberMax)}
          disabled={durationMs <= 0}
          style={cssVariables({
            "--range-fill": `${scrubberMax > 0 ? (Math.min(positionMs, scrubberMax) / scrubberMax) * 100 : 0}%`,
          })}
          aria-label={t("aria.scrubber")}
          aria-valuetext={formatClock(positionMs)}
          onChange={(event) => actions.seekToMs(Number(event.currentTarget.value))}
        />

        {/* 控制栏 */}
        <div className="luna-playback__controls" role="group" aria-label={t("aria.controls")}>
          {/* 走带组：后退 10 秒 · 播放/暂停 · 前进 10 秒 · 倍速（PRD-04 §3.4）。
              ±10 秒是回放里最高频的动作（"刚那句没听清"），此前只能拖进度条。 */}
          <div className="luna-playback__transport-group">
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-playback__btn--skip"
              disabled={durationMs <= 0}
              aria-label={t("action.back10")}
              title={t("action.back10")}
              onClick={() => actions.seekToMs(Math.max(0, positionMs - SKIP_MS))}
            >
              {t("action.back10Short")}
            </button>
            <button
              type="button"
              className="luna-btn luna-btn--primary"
              onClick={() => actions.togglePlay()}
              aria-label={playing ? t("action.pause") : t("action.play")}
              aria-pressed={playing}
            >
              {/* 非颜色-only：播放/暂停靠**形状**区分，不是只改颜色。图形走 Icon 组件——
                  字符 ▶/⏸ 的字形、基线与粗细随系统字体变，与整套图标对不齐。 */}
              {playing ? <PauseIcon size={14} /> : <PlayIcon size={14} />}
              <span className="luna-playback__btn-label">
                {playing ? t("action.pause") : t("action.play")}
              </span>
            </button>
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-playback__btn--skip"
              disabled={durationMs <= 0}
              aria-label={t("action.forward10")}
              title={t("action.forward10")}
              onClick={() => actions.seekToMs(Math.min(durationMs, positionMs + SKIP_MS))}
            >
              {t("action.forward10Short")}
            </button>
            {/* 倍速：一枚胶囊，点一下轮到下一档（对标 Teams / Podcasts）——五颗并排的按钮
                把最不常改的设置摆成了最占地的控件。 */}
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-playback__rate-cycle"
              aria-label={t("speed.cycle", { rate: playbackRate })}
              title={t("speed.cycle", { rate: playbackRate })}
              onClick={() => {
                const next = PLAYBACK_RATES[(PLAYBACK_RATES.indexOf(playbackRate) + 1) % PLAYBACK_RATES.length] as PlaybackRate;
                actions.setPlaybackRate(next);
              }}
            >
              {t("speed.option", { rate: playbackRate })}
            </button>
          </div>

          {/* 双语切换（译文缺失时禁用并提示先翻译） */}
          <div className="luna-playback__bilingual" role="group" aria-label={t("bilingual.label")}>
            {bilingualOptions.map(({ mode, label, disabled }) => (
              <button
                key={mode}
                type="button"
                className="luna-btn luna-btn--secondary"
                aria-pressed={bilingualMode === mode}
                disabled={disabled}
                title={disabled ? t("bilingual.needTranslation") : undefined}
                onClick={() => actions.setBilingualMode(mode)}
              >
                {label}
              </button>
            ))}
          </div>

          {/* 校对（PRD-04-031：进入整篇校对编辑器）——用用户的词，不叫"文本编辑模式" */}
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            aria-pressed={fullEditMode}
            disabled={cues.length === 0}
            onClick={() => {
              if (fullEditMode) void finishFullEdit();
              else enterFullEdit();
            }}
          >
            {t("edit.modeToggle")}
          </button>

          {/* follow-mode 指示器：手动滚动后在这一行给「回到当前行」（整篇编辑态下不打扰） */}
          {follow.mode === "manual" && !fullEditMode && (
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-btn--sm luna-playback__return-btn"
              onClick={() => actions.returnToCurrent()}
              aria-label={t("action.returnToCurrent")}
            >
              {t("action.returnToCurrent")}
            </button>
          )}

          {/* 视图内查找：在这一篇里找"那句话"。命中只高亮、不隐藏别的行——转写的上下文就是价值。
              Enter / Shift+Enter 在命中之间跳，Esc 清除。 */}
          <div className="luna-playback__find" role="search" aria-label={t("find.label")}>
            {/* 与侧栏 / 全库搜索同一个查询框原语（`QueryBox`）：同一副外形、同一枚清空钮。 */}
            <QueryBox
              value={findQuery}
              onChange={setFindQuery}
              placeholder={t("find.placeholder")}
              ariaLabel={t("find.label")}
              className="luna-playback__find-box"
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  stepFind(event.shiftKey ? -1 : 1);
                } else if (event.key === "Escape" && findQuery) {
                  event.preventDefault();
                  setFindQuery("");
                }
              }}
            >
              {findQuery.trim() && (
                <span className="luna-playback__find-count" aria-live="polite">
                  {findMatches.length > 0
                    ? t("find.count", { index: findIndex + 1, total: findMatches.length })
                    : t("find.none")}
                </span>
              )}
            </QueryBox>
            {findQuery.trim() && (
              <>
                <IconButton
                  density="sm"
                  label={t("find.prev")}
                  icon={<ChevronUpIcon size={14} />}
                  disabled={findMatches.length === 0}
                  onClick={() => stepFind(-1)}
                />
                <IconButton
                  density="sm"
                  label={t("find.next")}
                  icon={<ChevronDownIcon size={14} />}
                  disabled={findMatches.length === 0}
                  onClick={() => stepFind(1)}
                />
              </>
            )}
          </div>
        </div>

        {/* 书签跳转条（PRD-04-005：可点击时间锚点） */}
        {bookmarks.length > 0 && (
          <div className="luna-playback__bookmarks" role="group" aria-label={t("bookmarks.label")}>
            {bookmarks.map(({ ms, note }) => (
              <button
                key={ms}
                type="button"
                className="luna-btn luna-btn--secondary luna-btn--sm luna-playback__bookmark-chip"
                onClick={() => actions.seekToMs(ms)}
                title={note ?? undefined}
                aria-label={
                  note
                    ? t("bookmarks.jumpNoted", { time: formatClock(ms), note })
                    : t("bookmarks.jump", { time: formatClock(ms) })
                }
              >
                <BookmarkIcon size={14} />
                {formatClock(ms)}
                {note ? <span className="luna-playback__bookmark-note">{note}</span> : null}
              </button>
            ))}
          </div>
        )}

        {/* 说话人名册条（PRD-04-056）：配色图例 + 全局改名入口；改名就地输入固定在此行 */}
        {renamingSpeaker ? (
          <form
            className="luna-playback__speakers"
            onSubmit={(event) => {
              event.preventDefault();
              void saveSpeakerRename();
            }}
          >
            <span className="luna-playback__speakers-label">
              {t("speaker.editLabel", {
                label: speakerLabel(renamingSpeaker.speaker),
              })}
            </span>
            <input
              ref={renameInputRef}
              className="luna-input luna-playback__speaker-input"
              value={renamingSpeaker.value}
              placeholder={t("speaker.placeholder")}
              aria-label={t("speaker.editLabel", {
                label: speakerLabel(renamingSpeaker.speaker),
              })}
              onChange={(event) => {
                const value = event.currentTarget.value;
                setRenamingSpeaker((prev) => (prev ? { ...prev, value } : prev));
              }}
              onKeyDown={(event) => {
                if (event.key === "Escape") {
                  event.preventDefault();
                  event.stopPropagation();
                  setRenamingSpeaker(null);
                }
              }}
            />
            <button
              type="submit"
              className="luna-btn luna-btn--primary luna-btn--sm"
            >
              {t("speaker.renameSave")}
            </button>
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-btn--sm"
              onClick={() => setRenamingSpeaker(null)}
            >
              {t("edit.cancel")}
            </button>
          </form>
        ) : (
          <SpeakerRoster
            roster={speakers}
            displayName={speakerLabel}
            onRename={startSpeakerRename}
          />
        )}


        {/* 错误/加载提示 */}
        {loading && (
          <p className="luna-playback__status" role="status">
            {downloadPercent === null
              ? t("status.loading")
              : t("status.downloading", { pct: downloadPercent })}
          </p>
        )}
        {error && (
          <p className="luna-playback__status luna-playback__status--error" role="alert">
            {t("status.error")}: {error}
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-btn--sm"
              onClick={() => actions.retry()}
            >
              {t("action.retry")}
            </button>
          </p>
        )}
        {editStatus && (
          <p className="luna-playback__status" role="status">
            {editStatus}
          </p>
        )}
        {/* 呈现控制行（PRD-04-058，2026-09-08 修订）：四种呈现下**位置不变**——此前它长在
            画面舞台里，随画面在四个位置之间跳。只有视频笔记才有这一行。 */}
        {isVideo && (
          <VideoStageControls
            layout={videoLayout}
            onLayoutChange={videoPrefs.setLayout}
            subtitlesEnabled={videoPrefs.subtitlesEnabled}
            onSubtitlesToggle={videoPrefs.setSubtitlesEnabled}
            subtitleSize={videoPrefs.subtitleSize}
            onSubtitleSizeChange={videoPrefs.setSubtitleSize}
          />
        )}
      </div>

      {/* 主体区（唯一的滚动区）：转写列表 + 视频舞台。分屏态并排、剧场态画面独占、悬浮态
          画面浮起、仅音频态只剩列表——布局全由 data 属性驱动，`<video>` 元素恒挂载不重建。 */}
      <div className="luna-playback__body" data-layout={isVideo ? videoLayout : "audio_only"}>
        {/* 空态与列表**二选一**：两者并存会各占一个网格位，分屏态下把画面挤出主行 */}
        {transcriptEmpty ? (
          <p className="luna-playback__empty" role="status">
            {t("status.noCues")}
          </p>
        ) : (
          /* role="listbox" + role="option" a11y pattern */
          <ol
            ref={listRef}
            className="luna-playback__cuelist"
            role="listbox"
            aria-label={t("aria.cueList")}
            aria-activedescendant={activeCueIndex >= 0 ? `cue-${activeCueIndex}` : undefined}
            data-full-edit={fullEditMode}
            tabIndex={0}
            onKeyDown={onKeyDown}
          >
            {cues.map((cue, index) => {
              const isActive = index === activeCueIndex;
              const isEditing = editingCueIndex === index;
              const draft = drafts[index] ?? cue.text;
              // 字幕质量（PRD-05-033）：超标即在行尾给一个安静的角标，不改变行的可读性。
              return (
                <li
                  key={`cue-${index}`}
                  id={`cue-${index}`}
                  ref={isActive ? activeCueRef : null}
                  role="option"
                  aria-selected={isActive}
                  className="luna-playback__cue"
                  data-quality={cue.issues.length > 0 ? "flagged" : undefined}
                  data-active={isActive}
                  data-editing={isEditing}
                  data-full-edit={fullEditMode}
                  data-match={findMatches.includes(index) || undefined}
                  data-hit={index === currentMatchCue || undefined}
                  title={fullEditMode ? undefined : t("edit.doubleClickHint")}
                  onClick={() => {
                    if (fullEditMode || isEditing) return;
                    actions.seekToCue(index);
                  }}
                  onDoubleClick={() => {
                    if (fullEditMode) return;
                    startCueEdit(index);
                  }}
                  onContextMenu={(event) => openCueMenu(event, index, cue)}
                >
                  {showQuality && cue.issues.length > 0 && (
                    <span
                      className="luna-playback__cue-quality"
                      title={cue.issues.map((issue) => t(`quality.${issue}`)).join(" · ")}
                      aria-label={cue.issues.map((issue) => t(`quality.${issue}`)).join(" · ")}
                    >
                      {cue.cps !== null && cue.issues.includes("cps")
                        ? t("quality.cpsBadge", { cps: Math.round(cue.cps) })
                        : t("quality.badge")}
                    </span>
                  )}
                  {/* 时间戳：整篇编辑态下为可点跳转按钮，常态为静态标记 */}
                  {fullEditMode ? (
                    <button
                      type="button"
                      className="luna-playback__cue-time luna-playback__cue-time--seek"
                      onClick={(event) => {
                        event.stopPropagation();
                        actions.seekToCue(index);
                      }}
                      aria-label={t("aria.seekTo", {
                        time: formatClock(cue.start_ms),
                      })}
                    >
                      {formatClock(cue.start_ms)}
                    </button>
                  ) : (
                    <span className="luna-playback__cue-time" aria-hidden="true">
                      {formatClock(cue.start_ms)}
                    </span>
                  )}
                  {/* 非颜色-only：active item 有边框 + 背景 (via CSS) + aria-selected */}
                  {isActive && !fullEditMode && (
                    <span className="luna-playback__cue-indicator" aria-hidden="true">
                      ▸
                    </span>
                  )}
                  {isEditing ? (
                    /* 单条就地编辑（双击入口）*/
                    <form
                      className="luna-playback__cue-editor"
                      onClick={(event) => event.stopPropagation()}
                      onSubmit={(event) => {
                        event.preventDefault();
                        void saveCueEdit(cue);
                      }}
                    >
                      <textarea
                        ref={editInputRef}
                        className="luna-playback__cue-textarea"
                        value={editingText}
                        rows={Math.min(4, Math.max(2, editingText.split("\n").length))}
                        aria-label={t("edit.label", {
                          time: formatClock(cue.start_ms),
                        })}
                        onChange={(event) => setEditingText(event.currentTarget.value)}
                        onKeyDown={(event) => onEditorKeyDown(event, cue)}
                        disabled={editingSaving}
                      />
                      {editingError && (
                        <span className="luna-playback__cue-edit-error" role="alert">
                          {editingError}
                        </span>
                      )}
                      <span className="luna-playback__cue-edit-actions">
                        {/* 删除本条（PRD-04-031）：清空文本不是删除——它只会留下一条空白字幕。
                            置于行首、与「保存」隔开，避免破坏性动作紧邻确认动作。 */}
                        {renderDeleteButton(index)}
                        <span className="luna-playback__cue-edit-hint">{t("edit.hint")}</span>
                        <button
                          type="button"
                          className="luna-btn luna-btn--secondary luna-btn--sm"
                          onClick={cancelCueEdit}
                          disabled={editingSaving}
                        >
                          {t("edit.cancel")}
                        </button>
                        <button
                          type="submit"
                          className="luna-btn luna-btn--primary luna-btn--sm"
                          disabled={editingSaving}
                        >
                          {editingSaving ? t("edit.saving") : t("edit.save")}
                        </button>
                      </span>
                    </form>
                  ) : fullEditMode ? (
                    /* 整篇校对：逐行可编辑源文（译文只读作校对参照）+ 结构动作（拆分/合并） */
                    <div className="luna-playback__cue-full">
                      {renderSpeakerBadge(index, cue)}
                      <textarea
                        ref={(el) => {
                          if (el) draftRefs.current.set(index, el);
                          else draftRefs.current.delete(index);
                        }}
                        className="luna-playback__cue-textarea luna-playback__cue-textarea--flow"
                        value={draft}
                        rows={draftRows(draft)}
                        aria-label={t("edit.label", {
                          time: formatClock(cue.start_ms),
                        })}
                        onClick={(event) => event.stopPropagation()}
                        onChange={(event) =>
                          setDrafts((prev) => ({
                            ...prev,
                            [index]: event.currentTarget.value,
                          }))
                        }
                        disabled={savingAll}
                      />
                      {cue.translation && bilingualMode !== "source" && (
                        <span className="luna-playback__cue-translation">{cue.translation}</span>
                      )}
                      {/* 结构动作：一条里两个人说话 → 光标处拆分；切碎的半句 → 与上一条合并；
                          整条不要（广告/口水/静音误识别）→ 删除。
                          mousedown 阻止默认以保住 textarea 里的光标（读 selectionStart 依赖它）。 */}
                      <span className="luna-playback__cue-tools">
                        <button
                          type="button"
                          className="luna-btn luna-btn--secondary luna-btn--sm"
                          disabled={speakerBusy || savingAll}
                          title={t("speaker.splitHint")}
                          onMouseDown={(event) => event.preventDefault()}
                          onClick={(event) => {
                            event.stopPropagation();
                            splitCueAtCaret(index);
                          }}
                        >
                          <ScissorsIcon size={12} />
                          {t("speaker.splitAction")}
                        </button>
                        <button
                          type="button"
                          className="luna-btn luna-btn--secondary luna-btn--sm"
                          disabled={index === 0 || speakerBusy || savingAll}
                          title={t("speaker.mergeHint")}
                          onMouseDown={(event) => event.preventDefault()}
                          onClick={(event) => {
                            event.stopPropagation();
                            mergeCueWithPrevious(index);
                          }}
                        >
                          <MergeIcon size={12} />
                          {t("speaker.merge")}
                        </button>
                        {renderDeleteButton(index)}
                      </span>
                    </div>
                  ) : (
                    /* 双语三态（PRD-04-025）：仅原文 / 仅译文（缺译回退原文）/ 双语对照 */
                    <span className="luna-playback__cue-text" data-bilingual={bilingualMode}>
                      {(bilingualMode !== "translation" || !cue.translation) && (
                        <span className="luna-playback__cue-source">
                          {cue.speaker && renderSpeakerBadge(index, cue)}
                          {renderFindable(cue.text)}
                        </span>
                      )}
                      {bilingualMode !== "source" && cue.translation && (
                        <span className="luna-playback__cue-translation">
                          {renderFindable(cue.translation)}
                        </span>
                      )}
                    </span>
                  )}
                </li>
              );
            })}
          </ol>
        )}

        {/* 视频舞台（PRD-04-058）：音频笔记时整体隐藏，`<video>` 仍在 DOM 内负责出声；
            呈现控制在走带区之下的固定一行（`VideoStageControls`），不随画面走。 */}
        <VideoStage mediaKind={mediaKind} layout={videoLayout} registerStage={registerStage} />
      </div>

      {/* 说话人指派菜单（PRD-04-056）：锚定被点击的徽标，目标 × 范围两维 */}
      {speakerMenu !== null && cues[speakerMenu.index] && (
        <SpeakerMenu
          anchor={{ x: speakerMenu.x, y: speakerMenu.y }}
          roster={speakers}
          current={cues[speakerMenu.index].speaker}
          nextSpeakerId={nextSpeakerId}
          displayName={speakerLabel}
          onAssign={(speaker, scope) => void assignSpeaker(speakerMenu.index, speaker, scope)}
          onRename={startSpeakerRename}
          onClose={() => setSpeakerMenu(null)}
          splitTip={!fullEditMode}
        />
      )}

      {/* 底栏：整篇编辑态 = 校对保存条；常态 = 处理动作（转写/翻译/细分/导出）+ cue 计数 */}
      {!loading && fullEditMode && (
        <footer className="luna-playback__footer luna-playback__footer--edit">
          <div className="luna-playback__edit-bar" role="group" aria-label={t("fullEdit.title")}>
            <span className="luna-playback__edit-bar-title">{t("fullEdit.title")}</span>
            <span
              className="luna-playback__edit-bar-dirty"
              data-dirty={dirtyCount > 0}
              role="status"
            >
              {dirtyCount > 0 ? t("fullEdit.dirty", { count: dirtyCount }) : t("fullEdit.clean")}
            </span>
            <span className="luna-playback__edit-bar-hint">{t("fullEdit.hint")}</span>
            {/* 放弃仅在有未保存改动时出现，避免误触丢失校对；「完成」= 保存并退出（安全默认） */}
            {dirtyCount > 0 && (
              <button
                type="button"
                className="luna-btn luna-btn--secondary"
                onClick={discardFullEdit}
                disabled={savingAll}
              >
                {t("fullEdit.discard")}
              </button>
            )}
            <button
              type="button"
              className="luna-btn luna-btn--primary"
              onClick={() => void finishFullEdit()}
              disabled={savingAll}
            >
              {savingAll ? t("fullEdit.saving") : t("fullEdit.done")}
            </button>
          </div>
        </footer>
      )}

      {!loading && !fullEditMode && (
        <footer className="luna-playback__footer">
          <div className="luna-playback__actions">
            {/* 字幕质检（PRD-05-033）默认收起：一枚开关说清有几条，点开才在逐行挂徽标。
                与其余动作同一行、同一尺寸——它是这一排里的一颗，不是一行说明文字。 */}
            {flaggedCount > 0 && (
              <button
                type="button"
                className="luna-btn luna-btn--secondary luna-playback__quality-toggle"
                aria-pressed={showQuality}
                title={t("quality.summary", { flagged: flaggedCount, total: cues.length })}
                onClick={() => setShowQuality((current) => !current)}
              >
                {showQuality ? t("quality.hide") : t("quality.show", { count: flaggedCount })}
              </button>
            )}
            {/* 二次转写与说话人细分要**读本地音频**，仅存链接的转写（PRD-05-006）没有，
                故这两个动作不渲染而不是渲染成禁用——点了才知道不行是误导；取回媒体的入口
                已在顶部说明区。**翻译只吃文本、导出只吃 cue**，二者不受影响照常可用。 */}
            {hasMedia && (
              /* 二次转写（PRD-04-030）：入队重跑 whisper；开启自动转译时其内部转写完成后再逐句译 */
              <button
                type="button"
                className="luna-btn luna-btn--secondary"
                onClick={() => void retranscribe()}
                disabled={Boolean(retranscribeTask)}
              >
                {actionLabel(retranscribeTask, t("secondPass.running"), t("secondPass.run"))}
              </button>
            )}

            {/* 二次翻译（PRD-04-034）：分段下拉选目标语言 + 入队触发，段控式一体化 */}
            <span
              className="luna-playback__translate"
              role="group"
              aria-label={t("translate.label")}
            >
              <span className="luna-playback__translate-label">{t("translate.targetPrefix")}</span>
              <SelectBox
                size="sm"
                className="luna-select--flush luna-playback__translate-target"
                value={translateTarget}
                disabled={Boolean(translateTask) || cues.length === 0}
                ariaLabel={t("translate.targetLabel")}
                onChange={(event) => setTranslateTarget(event.currentTarget.value)}
              >
                <option value="smart">{t("translate.smart")}</option>
                {translateTargets.map((lang) => (
                  <option key={lang} value={lang}>
                    {tCommon(`lang.${lang}`)}
                  </option>
                ))}
              </SelectBox>
              <button
                type="button"
                className="luna-btn luna-btn--secondary luna-playback__translate-run"
                onClick={() => void enqueue(commands.enqueueTranslate(noteRel, translateTarget))}
                disabled={Boolean(translateTask) || cues.length === 0}
              >
                {actionLabel(
                  translateTask,
                  t("translate.running"),
                  hasTranslation ? t("translate.rerun") : t("translate.run"),
                )}
              </button>
            </span>

            {hasMedia && (
              /* 说话人细分（PRD-04-054）：入队本地 diarization（同样要读音频） */
              <button
                type="button"
                className="luna-btn luna-btn--secondary"
                onClick={() => void enqueue(commands.enqueueDiarize(noteRel))}
                disabled={Boolean(diarizeTask) || cues.length === 0}
              >
                {actionLabel(diarizeTask, t("diarize.running"), t("diarize.run"))}
              </button>
            )}

            {/* 导出菜单（PRD-04 §3.4）：内容跟随双语显示模式。
                此前是 `<details>` + `role="menu"` 自绘的第三份菜单——声明了菜单角色却
                没有 Esc、没有方向键。已收编到全应用唯一的菜单实现（`useContextMenu`）。 */}
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              disabled={cues.length === 0 || exporting}
              aria-haspopup="menu"
              onClick={(event) => {
                contextMenu.open(
                  event,
                  buildMenu([
                    ...TRANSCRIPT_EXPORT_FORMATS.map((format) => ({
                      kind: "action" as const,
                      id: format,
                      label: t(`transcribe:format.${format}`),
                      onSelect: () => void runExport(format),
                    })),
                    // 时移 / 折行 / BOM / 说话人前缀（PRD-11-004/007）：应用级同一张对话框。
                    {
                      kind: "action" as const,
                      id: "export-options",
                      label: tTranscribe(
                        exportOptionsCustomized(exportOptions)
                          ? "exportOptions.menuCustomized"
                          : "exportOptions.menu",
                      ),
                      onSelect: () => requestExportOptions(),
                    },
                  ]),
                );
              }}
            >
              {exporting ? t("export.running") : t("export.label")}
            </button>
          </div>

          {/* 本录音在处理队列中的内联状态（进度条/排队/失败与全局队列面板同源呈现） */}
          <div className="luna-playback__action-status">
            {runningTask && <ProcessingProgress task={runningTask} />}
            {!runningTask && anyActive && (
              <span className="luna-playback__second-pass-progress" role="status">
                {t("queuedHint")}
              </span>
            )}
            {lastFailed && lastFailed.status.state === "failed" && (
              <span className="luna-playback__second-pass-error" role="alert">
                {t("processFailed")}
                <small>{taskFailureText(lastFailed.status.data.code, tTranscribe)}</small>
              </span>
            )}
            {enqueueError && (
              <span className="luna-playback__second-pass-error" role="alert">
                {enqueueError}
              </span>
            )}
          </div>

          {cues.length > 0 && (
            <span className="luna-playback__cue-count">
              {t("status.cueCount", { count: cues.length })}
            </span>
          )}
        </footer>
      )}
    </section>
  );
}
