/**
 * 常驻播放宿主（ADR-118 / PRD-04-064）。
 *
 * **它解决的是一条结构问题，不是一个显示问题**：此前 `<video>` 元素与全部播放状态
 * 都住在 `SyncPlaybackView` 里，而空间切换是条件渲染——切到笔记空间那一刻整棵子树卸载，
 * `usePlayback` 的清理把音源 `removeAttribute("src")` 掉，播放必然中断（真机 2026-09-08
 * 上报：「切到别的标签页播放就断了，应该像 YouTube 那样后台继续」）。
 *
 * 于是把"谁拥有媒体"上提一层：
 * - **元素与状态归宿主**，宿主挂在工作区的空间条件渲染**之外**，随工作区常驻；
 * - **回放视图降为投影**：它不再持有 `usePlayback`，只从上下文取状态、并把画面槽位
 *   （`.luna-video-stage__frame`）注册给宿主；
 * - 宿主把常驻画面层**贴到已注册槽位的矩形上**。因此分屏 / 剧场 / 悬浮小窗三种布局
 *   不需要宿主知道任何布局细节——槽位在哪画面就在哪，拖动小窗时画面跟着走。
 * - 槽位没注册（用户离开了回放视图）时，画面层收起、声音继续，底部出现「正在播放」条。
 *
 * **不引状态库**（TDD-07 §5）：这里是 React context + 既有 `usePlayback`，所有权与
 * 清除边界都在本文件里一眼看完。播放头仍是前端瞬时态、仍由 `timeupdate` 本地二分查找
 * 驱动，**不占高频 IPC 通道**（ADR-011 不受影响）；cue 与媒体解析仍归 Rust（ADR-003）。
 *
 * **画面覆盖物（播放键 / 字幕）必须与 `<video>` 同住宿主**：画面层是 `position: fixed`，
 * 留在视图里的覆盖物会被它压在下面——那不是层级调一调的事，是两个栈上下文的事。
 */

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useTranslation } from "react-i18next";
import { cssVariables } from "../../lib/css-variables";
import {
  CloseIcon,
  ExpandIcon,
  MusicIcon,
  PauseIcon,
  PlayIcon,
} from "../../components/ui/Icon";
import {
  usePlayback,
  type PlaybackActions,
  type PlaybackState,
} from "./use-playback";
import { events } from "../../ipc/bindings";
import { intentOnOpen, shouldAutoplay, type OpenOptions, type PlayIntent } from "./autoplay";
import { useVideoPrefs, type VideoPrefs } from "./use-video-prefs";
import { applyStageRegistration, NO_STAGE, type StageRegistration } from "./stage-registration";
import {
  clampFloatPosition,
  defaultFloatPosition,
  subtitleLines,
  type FloatPosition,
  type VideoLayout,
} from "./video-stage";
import { shouldStartDrag } from "../../lib/pointer-drag";
import { NowPlayingBar } from "./NowPlayingBar";
import { goTo } from "../../lib/navigate";
import { PLAYBACK_AT_EVENT, type PlaybackAtDetail } from "../../lib/app-events";

/**
 * 元数据到达后再落 seek。
 *
 * 装载完成（`loading` 落下）那一刻媒体刚换源、`readyState` 还是 `HAVE_NOTHING`，此时
 * 直接写 `currentTime` 靠的是引擎对"默认起播位置"的实现，不同 WebKit 版本不一致；
 * 等 `loadedmetadata` 是规范里唯一可靠的落位时机。监听器只认**挂上时的那份音源**：
 * 用户在元数据到达前又换了篇，这次 seek 就作废，不能打到下一篇上。
 */
function seekWhenReady(media: HTMLMediaElement, ms: number, seek: (ms: number) => void) {
  if (media.readyState >= HTMLMediaElement.HAVE_METADATA) {
    seek(ms);
    return;
  }
  const src = media.getAttribute("src");
  media.addEventListener(
    "loadedmetadata",
    () => {
      if (media.getAttribute("src") === src) seek(ms);
    },
    { once: true },
  );
}

/** 单击画面的延后执行窗口（ms）：双击到达即撤销该次播放/暂停。 */
const CLICK_DEBOUNCE_MS = 220;

/** 后台悬浮小窗的尺寸（16:9，与视图内小窗同宽）——用于首次定位与视口夹取。 */
const BACKGROUND_FLOAT_SIZE = { width: 360, height: 203 };

/** 画面层的三种形态，见 `stageMode` 的注释。 */
type PlaybackStageMode = "stage" | "background-float" | "hidden";

export interface PlaybackHostValue {
  /** 当前装载的笔记；`null` = 空载。 */
  noteRel: string | null;
  playbackState: PlaybackState;
  actions: PlaybackActions;
  /** 视频呈现偏好：宿主与回放视图共用同一份订阅，不各订一次。 */
  videoPrefs: VideoPrefs;
  /**
   * 装载一篇。同一篇重复调用是空操作，不会打断正在进行的播放。
   *
   * `play: true` = 装好即起播（编辑面的「在此播放」）。起播必须**等媒体就绪**，故它是
   * 一枚意图而不是一次调用：装载是异步的，`open()` 返回时音源还没挂上。`seekMs` 同理
   * 随意图走（时间戳行的「从这一句开始听」），不能在 `open()` 之后自己再 seek 一次。
   */
  open: (noteRel: string, options?: OpenOptions) => void;
  /** 卸载当前媒体（迷你条的关闭键）。 */
  close: () => void;
  /**
   * 回放视图把画面槽位交给宿主；卸载时传 `null`。
   *
   * `stageId` 是**这一个槽位实例**的身份（`useId()`）：只有当前持有者能注销自己，
   * 否则分屏切布局时旧槽位迟到的卸载回调会把新槽位抹掉（`applyStageRegistration`）。
   */
  registerStage: (stageId: string, element: HTMLElement | null) => void;
  /**
   * 编辑器登记"我正开着哪一篇"；卸载/换篇时传 `null`。
   *
   * 宿主只用它回答一个问题：**「跟随」这枚开关此刻有没有作用对象**。开着别的空间时
   * 跟随无处可跟，那枚开关就不该出现在「正在播放」条上——一个按下去什么也不发生的
   * 开关比没有这枚开关更糟。
   */
  registerEditorNote: (noteRel: string | null) => void;
  /** 正在编辑的这一篇就是正在放的这一篇（「跟随」开关的出现条件）。 */
  editorShowsPlayingNote: boolean;
  /** 播放时把编辑器滚到当前句（会话内状态，默认关——编辑中被抢走滚动位置最恼人）。 */
  followInEditor: boolean;
  setFollowInEditor: (follow: boolean) => void;
  /** 「回到回放视图」的落点（`recording://` / `transcribe://` 深链已有解析器）。 */
  reopenHref: string | null;
}

const PlaybackHostContext = createContext<PlaybackHostValue | null>(null);

/** 取常驻播放宿主。**必须在 `PlaybackProvider` 之内调用**。 */
export function usePlaybackHost(): PlaybackHostValue {
  const value = useContext(PlaybackHostContext);
  if (value === null) {
    throw new Error("usePlaybackHost 必须在 PlaybackProvider 之内使用");
  }
  return value;
}

/**
 * 取常驻播放宿主，**没有宿主时返回 `null`** 而不是抛错。
 *
 * 给那些"主窗口里有播放能力、别处没有也照常工作"的组件用——编辑器就是这样一个：它同时
 * 服务主窗口与副库窗口（PRD-06-050），后者没有播放宿主。就地起播在那里降级为不出现，
 * 而不是让整扇窗崩掉。
 */
export function usePlaybackHostOptional(): PlaybackHostValue | null {
  return useContext(PlaybackHostContext);
}

/** 槽位矩形（视口坐标）；`null` = 没有注册槽位，即用户不在回放视图里。 */
type StageRect = { x: number; y: number; width: number; height: number } | null;

function sameRect(a: StageRect, b: StageRect): boolean {
  if (a === null || b === null) return a === b;
  return (
    a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height
  );
}

export function PlaybackProvider({ children }: { children: ReactNode }) {
  const [noteRel, setNoteRel] = useState<string | null>(null);
  const { playbackState, actions, mediaRef } = usePlayback(noteRel);
  const videoPrefs = useVideoPrefs(noteRel !== null);
  const [stage, setStage] = useState<StageRegistration>(NO_STAGE);
  const stageEl = stage.element;
  const [stageRect, setStageRect] = useState<StageRect>(null);

  /**
   * 待执行的起播意图：**哪一篇**要起播，`null` = 没有。
   *
   * 它必须带笔记名并且**用完就清**（真机 2026-09-08 上报「切到转写和录音时有时候会
   * 自动播放」）。此前这里是一个只增不减的 nonce：第一次点过「在此播放」之后它就永远
   * 非零，而 effect 依赖里有 `loading`——此后每打开一篇新的录音/转写，`loading` 落下
   * 都会再次满足条件，于是"点开看看"变成了"点开就放"。
   * `nonce` 仍在，用来表达"同一篇再点一次也要重新起播"；判据则是笔记名 + 一次性消费。
   */
  const [pendingPlay, setPendingPlay] = useState<PlayIntent | null>(null);

  const open = useCallback((next: string, options?: OpenOptions) => {
    // 同一篇不重新装载：回放视图重挂（切标签页回来）不该把正在听的那段打回开头。
    setNoteRel((prev) => (prev === next ? prev : next));
    // 意图的去留（含"换到别篇即作废"）在纯函数里，连同理由一起被测试钉住。
    setPendingPlay((prev) => intentOnOpen(prev, next, options));
  }, []);

  // 引文出处带时刻的链接（ADR-162 ⑭）：从那一刻开始播放那一篇。
  useEffect(() => {
    const onPlayAt = (event: Event) => {
      const detail = (event as CustomEvent<PlaybackAtDetail>).detail;
      if (detail) open(detail.path, { play: true, seekMs: detail.ms });
    };
    window.addEventListener(PLAYBACK_AT_EVENT, onPlayAt);
    return () => window.removeEventListener(PLAYBACK_AT_EVENT, onPlayAt);
  }, [open]);

  // 起播意图 → 就绪即播，然后**立刻清掉**。两条路都要覆盖：换篇（等 `loading` 落下）
  // 与同一篇再点一次（`nonce` 变了而 `loading` 一直是 false）。
  const seekToMs = actions.seekToMs;
  const { loadedNote, loading, hasMedia: loadedHasMedia } = playbackState;
  useEffect(() => {
    // 判据在纯函数里（`autoplay.ts`），连同"为什么它必须一次性"一起被测试钉住。
    // 「装的是哪一篇」看 `loadedNote`（装载完成才写回）而不是 `noteRel`：换篇那一帧
    // `noteRel` 已是新篇而 `loading` 还是旧值，看 `noteRel` 会在音源换上之前就把意图
    // 消费掉——`play()` 打在刚卸掉音源的元素上，新篇装好后没人再起播。
    const intent = pendingPlay;
    const go =
      intent !== null &&
      shouldAutoplay({ intent, loadedNote, loading, hasMedia: loadedHasMedia });
    if (!go) return;
    setPendingPlay(null);
    const media = mediaRef.current;
    if (!media) return;
    if (intent.seekMs !== undefined) seekWhenReady(media, intent.seekMs, seekToMs);
    void media.play();
  }, [pendingPlay, loadedNote, loading, loadedHasMedia, mediaRef, seekToMs]);

  const close = useCallback(() => setNoteRel(null), []);

  // ── 出声互斥（PRD-04-065）───────────────────────────────────────────────────
  //
  // 全局同一时刻只允许一路出声。判据取 **Rust 权威事件**而不是"谁点了那颗按钮"：录音可以
  // 由全局快捷键、悬浮窗或托盘发起，把互斥挂在工作区的按钮上会漏掉那几条路。
  // 三个来源各自的理由：
  //   · 录音进行中 —— 扬声器里的回放会被麦克风采回去，直接污染这一次录音；
  //   · 语音速记 HUD —— 同上，且用户此刻正对着麦克风说话；
  //   · 全文朗读 —— 两路人声同时讲，两边都听不清。
  // 只**暂停**不卸载：录完回来按一下就接着听。PRD-07 §5 那条开放问题自此有答案。
  useEffect(() => {
    const pauseSelf = () => {
      const media = mediaRef.current;
      if (media && !media.paused) media.pause();
    };
    const pending = [
      events.recordingStateChanged.listen((event) => {
        if (event.payload.status === "recording") pauseSelf();
      }),
      events.voiceInputHudOpened.listen(() => pauseSelf()),
      events.ttsState.listen((event) => {
        if (event.payload.status === "speaking") pauseSelf();
      }),
    ];
    return () => {
      for (const unlisten of pending) void unlisten.then((dispose) => dispose());
    };
  }, [mediaRef]);

  // 登记带身份：**只有当前持有者能注销自己**。ref 回调的 `null` 不携带身份，而分屏 /
  // 剧场之间切布局时 React 可以先挂新槽位、再卸旧槽位——那次迟到的 `null` 属于旧槽位，
  // 照单全收会把活着的那个抹掉，宿主从此以为自己不在回放视图（既不给后台悬浮小窗，
  // 也不给「正在播放」条）。判定在 `applyStageRegistration`，此处只落状态。
  const registerStage = useCallback((stageId: string, element: HTMLElement | null) => {
    setStage((current) => {
      const next = applyStageRegistration(current, stageId, element);
      if (next.element === null && current.element !== null) setStageRect(null);
      return next;
    });
  }, []);

  const [editorNote, setEditorNote] = useState<string | null>(null);
  const registerEditorNote = useCallback((next: string | null) => setEditorNote(next), []);
  // 跟随刻意**不持久化**：它是"我这会儿在听"的临时姿态，不是一条偏好。下次打开应用
  // 又从"不抢滚动"开始，符合默认安全的一侧。
  const [followInEditor, setFollowInEditor] = useState(false);

  // ── 槽位跟踪 ───────────────────────────────────────────────────────────────
  //
  // 用 rAF 逐帧读矩形而不是 `ResizeObserver`：槽位的**位置**同样会变而尺寸不变
  // （悬浮小窗被拖动、主侧栏被拖宽、布局在分屏/剧场间切换），而 ResizeObserver 只报尺寸。
  // 代价是一次 `getBoundingClientRect()` / 帧，且**只在回放视图挂着时跑**——离开这个空间
  // 就没有槽位，循环随之停掉。写 state 前先比对，矩形不动就不触发重渲染。
  useLayoutEffect(() => {
    if (stageEl === null) return;
    let frame = 0;
    let last: StageRect = null;
    const tick = () => {
      const box = stageEl.getBoundingClientRect();
      const next = { x: box.x, y: box.y, width: box.width, height: box.height };
      if (!sameRect(last, next)) {
        last = next;
        setStageRect(next);
      }
      frame = window.requestAnimationFrame(tick);
    };
    frame = window.requestAnimationFrame(tick);
    return () => window.cancelAnimationFrame(frame);
  }, [stageEl]);

  const inPlaybackView = stageEl !== null;
  const isVideo = playbackState.mediaKind === "video";
  const hasMedia = noteRel !== null && playbackState.hasMedia;

  /**
   * 画面层此刻的三种形态：
   * - `stage`：贴在回放视图的槽位上（分屏 / 剧场 / 视图内悬浮小窗都走这一档）；
   * - `background-float`：已离开回放视图，用户选了"悬浮小窗跟着走"（PRD-04-067）；
   * - `hidden`：只留声音——元素仍在渲染树里出声，这正是"切走仍在放"的实现方式。
   */
  const stageMode: PlaybackStageMode = !isVideo
    ? "hidden"
    : inPlaybackView
      ? videoPrefs.layout === "audio_only"
        ? "hidden"
        : "stage"
      : hasMedia && videoPrefs.backgroundVideo === "float"
        ? "background-float"
        : "hidden";

  // 后台悬浮小窗的位置（会话内有效；`null` = 尚未定位，取右下角默认位）。
  const [floatPos, setFloatPos] = useState<FloatPosition | null>(null);
  const floatPosRef = useRef<FloatPosition | null>(null);
  floatPosRef.current = floatPos;

  // 进入后台悬浮态定位一次，之后保留用户拖到的位置；窗口缩放后按实测尺寸重夹，
  // 否则缩小窗口会把小窗留在视口外找不回来（与视图内小窗同一条 `clampFloatPosition`）。
  useEffect(() => {
    if (stageMode !== "background-float") return;
    const settle = () => {
      const viewport = { width: window.innerWidth, height: window.innerHeight };
      const size = {
        width: BACKGROUND_FLOAT_SIZE.width,
        height: BACKGROUND_FLOAT_SIZE.height,
      };
      const current = floatPosRef.current;
      setFloatPos(
        current
          ? clampFloatPosition(current, size, viewport)
          : defaultFloatPosition(size, viewport),
      );
    };
    settle();
    window.addEventListener("resize", settle);
    return () => window.removeEventListener("resize", settle);
  }, [stageMode]);

  const editorShowsPlayingNote = noteRel !== null && editorNote === noteRel;

  const reopenHref = useMemo(() => {
    if (noteRel === null) return null;
    // 录音与转写共用同一个回放视图，深链前缀按笔记落点区分（PRD-04-020a ④）。
    return `${noteRel.startsWith("Recordings/") ? "recording" : "transcribe"}://${noteRel}`;
  }, [noteRel]);

  const value = useMemo<PlaybackHostValue>(
    () => ({
      noteRel,
      playbackState,
      actions,
      videoPrefs,
      open,
      close,
      registerStage,
      registerEditorNote,
      editorShowsPlayingNote,
      followInEditor,
      setFollowInEditor,
      reopenHref,
    }),
    [
      noteRel,
      playbackState,
      actions,
      videoPrefs,
      open,
      close,
      registerStage,
      registerEditorNote,
      editorShowsPlayingNote,
      followInEditor,
      reopenHref,
    ],
  );

  return (
    <PlaybackHostContext.Provider value={value}>
      {children}
      <PlaybackStageLayer
        mediaRef={mediaRef}
        rect={stageRect}
        mode={stageMode}
        floatPos={floatPos}
        onFloatPosChange={setFloatPos}
        layout={videoPrefs.layout}
        onLayoutChange={videoPrefs.setLayout}
      />
      {/* 小窗与播放条**不同时出现**（真机 2026-09-08）：两者答的是同一个问题
          「现在在放什么、怎么控制它」。小窗在场时字幕与控制都归小窗，屏幕底下不该再压
          一条重复的横幅。 */}
      {hasMedia && !inPlaybackView && stageMode !== "background-float" && <NowPlayingBar />}
    </PlaybackHostContext.Provider>
  );
}

/**
 * 常驻画面层：`<video>` 与压在画面上的两件覆盖物（起播键 / 字幕）住在这里，**永不卸载**。
 *
 * 它贴在回放视图注册的槽位上；没有槽位（或音频笔记 / 仅音频档）时整层收起——元素仍在
 * DOM 里出声，这正是"切走仍在放"的实现方式。
 */
function PlaybackStageLayer({
  mediaRef,
  rect,
  mode,
  floatPos,
  onFloatPosChange,
  layout,
  onLayoutChange,
}: {
  mediaRef: React.RefObject<HTMLVideoElement>;
  rect: StageRect;
  mode: PlaybackStageMode;
  floatPos: FloatPosition | null;
  onFloatPosChange: (position: FloatPosition) => void;
  layout: VideoLayout;
  onLayoutChange: (layout: VideoLayout) => void;
}) {
  const { t } = useTranslation("playback");
  const { playbackState, actions, videoPrefs, close, reopenHref } = usePlaybackHost();
  const clickTimerRef = useRef<number | null>(null);
  const layerRef = useRef<HTMLDivElement | null>(null);
  const visible = mode !== "hidden";
  const floating = mode === "background-float";

  /**
   * 拖动后台悬浮小窗。与视图内小窗同一套契约（`shouldStartDrag` 阈值 + 达阈值才捕获
   * 指针）——早先在 `pointerdown` 即捕获，捕获会把随后的 click 重定向到容器上，于是
   * 小窗里所有控件全部失效。这里唯一的控件是起播键，同样得保住。
   */
  function startFloatDrag(event: React.PointerEvent<HTMLDivElement>) {
    if (!floating || event.button !== 0) return;
    if ((event.target as HTMLElement).closest("button")) return;
    const layer = layerRef.current;
    const rectNow = layer?.getBoundingClientRect();
    if (!layer || !rectNow) return;
    const offsetX = event.clientX - rectNow.left;
    const offsetY = event.clientY - rectNow.top;
    const size = { width: rectNow.width, height: rectNow.height };
    const origin = { x: event.clientX, y: event.clientY };
    const pointerId = event.pointerId;
    let started = false;

    const onMove = (moveEvent: PointerEvent) => {
      if (!started) {
        if (!shouldStartDrag(moveEvent.clientX - origin.x, moveEvent.clientY - origin.y)) return;
        started = true;
        layer.setPointerCapture(pointerId);
      }
      onFloatPosChange(
        clampFloatPosition({ x: moveEvent.clientX - offsetX, y: moveEvent.clientY - offsetY }, size, {
          width: window.innerWidth,
          height: window.innerHeight,
        }),
      );
    };
    const onEnd = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onEnd);
      window.removeEventListener("pointercancel", onEnd);
      if (started && layer.hasPointerCapture(pointerId)) layer.releasePointerCapture(pointerId);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onEnd);
    window.addEventListener("pointercancel", onEnd);
  }

  useEffect(
    () => () => {
      if (clickTimerRef.current !== null)
        window.clearTimeout(clickTimerRef.current);
    },
    [],
  );

  /** 画面单击：延后执行播放/暂停，等一等看是不是双击（切布局）。 */
  function onFrameClick() {
    if (!visible || clickTimerRef.current !== null) return;
    clickTimerRef.current = window.setTimeout(() => {
      clickTimerRef.current = null;
      actions.togglePlay();
    }, CLICK_DEBOUNCE_MS);
  }

  /** 画面双击：撤销待执行的播放/暂停，改为剧场 ↔ 分屏来回。 */
  function onFrameDoubleClick() {
    if (!visible) return;
    if (clickTimerRef.current !== null) {
      window.clearTimeout(clickTimerRef.current);
      clickTimerRef.current = null;
    }
    onLayoutChange(layout === "theater" ? "split" : "theater");
  }

  const lines = videoPrefs.subtitlesEnabled && visible
    ? subtitleLines(
        playbackState.activeCueIndex >= 0
          ? (playbackState.cues[playbackState.activeCueIndex] ?? null)
          : null,
        playbackState.bilingualMode,
      )
    : null;
  const hasSubtitle = Boolean(lines?.source || lines?.translation);

  return (
    <div
      ref={layerRef}
      className="luna-playback-stage-layer"
      data-mode={mode}
      data-visible={visible}
      data-float-pending={floating && floatPos === null}
      data-subtitle-size={videoPrefs.subtitleSize}
      aria-label={floating ? t("video.title") : undefined}
      aria-hidden={visible ? undefined : true}
      onPointerDown={startFloatDrag}
      style={cssVariables({
        "--playback-stage-x": rect ? `${rect.x}px` : undefined,
        "--playback-stage-y": rect ? `${rect.y}px` : undefined,
        "--playback-stage-w": rect ? `${rect.width}px` : undefined,
        "--playback-stage-h": rect ? `${rect.height}px` : undefined,
        "--video-float-x": floatPos ? `${floatPos.x}px` : undefined,
        "--video-float-y": floatPos ? `${floatPos.y}px` : undefined,
      })}
    >
      <video
        ref={mediaRef}
        className="luna-video-stage__video"
        crossOrigin="anonymous"
        playsInline
        // 原生控件与回放视图自有的播放/进度/倍速控制条重复，且会压住字幕层 → 不启用。
        controls={false}
        aria-label={visible ? t("video.title") : undefined}
        onClick={onFrameClick}
        onDoubleClick={onFrameDoubleClick}
      />
      {visible && !playbackState.playing && (
        <button
          type="button"
          className="luna-video-stage__play"
          aria-label={t("action.play")}
          onClick={() => actions.togglePlay()}
        >
          <PlayIcon size={20} />
        </button>
      )}
      {/* 小窗自带的紧凑控制条（悬停显形）：小窗在场时播放条已让位，播放/暂停、回到回放、
          只留声音这三件事必须在这里够得着，否则画面就成了一块关不掉也停不下的浮层。 */}
      {floating && (
        <div className="luna-playback-stage-layer__controls">
          <button
            type="button"
            className="luna-video-stage__btn"
            aria-label={playbackState.playing ? t("action.pause") : t("action.play")}
            title={playbackState.playing ? t("action.pause") : t("action.play")}
            onClick={() => actions.togglePlay()}
          >
            {playbackState.playing ? <PauseIcon size={13} /> : <PlayIcon size={13} />}
          </button>
          <button
            type="button"
            className="luna-video-stage__btn"
            aria-label={t("nowPlaying.pictureHide")}
            title={t("nowPlaying.pictureHide")}
            onClick={() => videoPrefs.setBackgroundVideo("audio_only")}
          >
            <MusicIcon size={13} />
          </button>
          {reopenHref !== null && (
            <button
              type="button"
              className="luna-video-stage__btn"
              aria-label={t("nowPlaying.reopen")}
              title={t("nowPlaying.reopen")}
              onClick={() => {
                goTo(reopenHref);
              }}
            >
              <ExpandIcon size={13} />
            </button>
          )}
          <button
            type="button"
            className="luna-video-stage__btn"
            aria-label={t("nowPlaying.close")}
            title={t("nowPlaying.close")}
            onClick={close}
          >
            <CloseIcon size={13} />
          </button>
        </div>
      )}

      {visible && hasSubtitle && (
        <div className="luna-video-stage__subtitles" aria-live="off">
          {lines?.source && (
            <p className="luna-video-stage__subtitle-line">{lines.source}</p>
          )}
          {lines?.translation && (
            <p className="luna-video-stage__subtitle-line luna-video-stage__subtitle-line--translation">
              {lines.translation}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
