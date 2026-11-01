/**
 * 同步回放 Hook（ADR-003 前端零业务）。
 *
 * 职责：
 * - 加载回放聚合包（loadSyncPlayback：分片表 + 合并 cues + 书签 + 拼接资源 id + 媒体类型）
 * - 管理媒体元素（通过 mediaRef 注入；多分片经媒体服务虚拟拼接呈现为单一连续时间轴）
 *   · 元素恒为 `<video>`：音频笔记同样由它承载（视频元素放音频天然可行），从而**换笔记
 *     不换元素**——音频/视频切换不会重建元素、丢播放状态（PRD-04-058）
 * - timeupdate (~4Hz) + rAF：二分查找当前 cue（O(log n)）
 * - Follow-mode 状态机（用户滚动 → manual；点击"回到当前" → following）
 * - 倍速（PRD-04 §3.4 对标 Teams 0.75–2x）与进度条 seek
 * - 键盘 seek：ArrowUp/Down + Enter = seek；Space = play/pause；Esc = 回到当前行
 * - 说话人改名 / 段人工校正（重指派·拆分·合并·删除）命令转发 + 成功后位置无损重载
 *   （PRD-04-031/054/056）
 *
 * 高频更新（timeupdate）不新增 IPC 调用（PRD-07 要求：不新增高频 IPC）。
 */

import { useCallback, useEffect, useReducer, useRef, useState } from "react";
import {
  commands,
  events,
  type PlaybackMediaKind,
  type SegmentEditDto,
  type SpeakerStatDto,
} from "../../ipc/bindings";
import {
  findActiveCueIndex,
  followReducer,
  normaliseCues,
  type CuePoint,
  type FollowState,
} from "../recording/recording-cue";
import { vaultMediaStreamUrl, vaultCompositeStreamUrl } from "../media/media-src";
import { vaultErrorText } from "../../lib/vault-error";
import { positionToStore, shouldPersist, type PersistMark } from "./playback-position";

// ─── 类型 ─────────────────────────────────────────────────────────────────────

/** 可选倍速档位（值作用于媒体元素 `playbackRate`）。 */
export const PLAYBACK_RATES = [0.75, 1, 1.25, 1.5, 2] as const;
export type PlaybackRate = (typeof PLAYBACK_RATES)[number];

/** 一条录音书签（PRD-04-005）：会话绝对 ms + 可选备注（来自正文 🔖 行）。 */
export interface PlaybackBookmark {
  ms: number;
  note: string | null;
}

export interface PlaybackState {
  cues: CuePoint[];
  /** 录音书签（会话绝对 ms，升序）。 */
  bookmarks: PlaybackBookmark[];
  activeCueIndex: number;
  /** 是否正在播放 */
  playing: boolean;
  /** 当前播放位置（ms） */
  positionMs: number;
  /** 总时长（ms；0 = 未知，等媒体元素 loadedmetadata 兜底） */
  durationMs: number;
  /** 当前倍速 */
  playbackRate: PlaybackRate;
  follow: FollowState;
  /** 错误信息（null = 无错误） */
  error: string | null;
  loading: boolean;
  /** 双语模式 */
  bilingualMode: BilingualMode;
  /** 说话人显示名映射（PRD-04-054：speaker id → 改名；事实源 = 笔记 frontmatter）。 */
  speakerNames: Record<string, string>;
  /** 说话人名册（PRD-04-056：按首次出现序 + 段数/时长；顺序即配色槽位）。 */
  speakers: SpeakerStatDto[];
  /** 「新建说话人」的下一个可用 id（后端按本录音命名家族派生）。 */
  nextSpeakerId: string;
  /** 媒体呈现类型（PRD-04-058）：`video` = 挂视频舞台并叠字幕；后端派生，前端不猜。 */
  mediaKind: PlaybackMediaKind;
  /**
   * 内容来源地址（PRD-05-006「从链接导入」的溯源；`null` = 录音或本地文件导入）。
   * 事实源 = 笔记 frontmatter `luna.source_url`，故随笔记迁移不丢。
   */
  sourceUrl: string | null;
  /**
   * 是否有**本地媒体**可播（PRD-05-006「仅存链接」的转写没有）。
   * 由后端的分片表派生——前端不猜，也不靠"音源加载失败"事后判断。
   */
  hasMedia: boolean;
  /**
   * **装载完成**的那一篇；换篇一开始就回到 `null`，聚合包到手才写回。
   *
   * 起播判据必须看它而不是 `noteRel`：换篇那一次提交里 `noteRel` 已经是新篇，而
   * `loading` / `hasMedia` 还是上一篇的旧值——只看 `noteRel` 会在音源还没换上时就把起播
   * 意图消费掉，`play()` 打在刚被卸掉音源的元素上，新篇装好后再也没人起播
   * （真机 2026-09-08「点了在此播放还要再点一次播放」）。
   */
  loadedNote: string | null;
}

/** 双语三态：仅原文 / 仅译文 / 双语对照（译文为空时禁用对照） */
export type BilingualMode = "source" | "translation" | "both";

export interface PlaybackActions {
  seekToMs: (ms: number) => void;
  seekToCue: (index: number) => void;
  togglePlay: () => void;
  returnToCurrent: () => void;
  setBilingualMode: (mode: BilingualMode) => void;
  setPlaybackRate: (rate: PlaybackRate) => void;
  /** 加载失败后重试（重载聚合包与音源）。 */
  retry: () => void;
  /** 用户手动滚动时调用 */
  onUserScroll: () => void;
  /** 说话人改名（PRD-04-054）：写笔记 frontmatter；空名恢复默认。返回错误串或 null。 */
  renameSpeaker: (speaker: string, name: string) => Promise<string | null>;
  /**
   * 转写段人工校正（PRD-04-031 删除 / PRD-04-056 重指派·拆分·合并）——命令为自写静默改
   * （不发 `vault://changed`），成功后本 Hook 主动重载聚合包（位置无损）。返回错误串或 null。
   */
  editSegment: (edit: SegmentEditDto) => Promise<string | null>;
}

export interface UsePlaybackResult {
  playbackState: PlaybackState;
  actions: PlaybackActions;
  /** 挂载媒体元素（恒 `<video>`，音频笔记亦由它播放）的 ref，由 SyncPlaybackView 注入。 */
  mediaRef: React.RefObject<HTMLVideoElement>;
}

/** 把这一篇此刻的位置写进设备用量库（PRD-04-068）；没装媒体或没时长时什么都不写。 */
function persistPosition(noteRel: string | null, media: HTMLMediaElement) {
  if (noteRel === null || !Number.isFinite(media.duration) || media.duration <= 0) return;
  void commands.playbackPositionSave(
    noteRel,
    positionToStore(media.currentTime * 1000, media.duration * 1000),
  );
}

// ─── Hook ─────────────────────────────────────────────────────────────────────

/**
 * @param noteRel 当前装载的笔记；`null` = 宿主空载（未装任何媒体），此时不发 IPC、
 *   不挂音源，状态回到初值。空载是**常驻宿主**的合法状态：宿主先于任何回放视图存在。
 */
export function usePlayback(noteRel: string | null): UsePlaybackResult {
  const [cues, setCues] = useState<CuePoint[]>([]);
  const [bookmarks, setBookmarks] = useState<PlaybackBookmark[]>([]);
  const [activeCueIndex, setActiveCueIndex] = useState(-1);
  const [playing, setPlaying] = useState(false);
  const [positionMs, setPositionMs] = useState(0);
  const [durationMs, setDurationMs] = useState(0);
  const [playbackRate, setPlaybackRateState] = useState<PlaybackRate>(1);
  const [follow, dispatchFollow] = useReducer(followReducer, { mode: "following" });
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [bilingualMode, setBilingualMode] = useState<BilingualMode>("source");
  const [speakerNames, setSpeakerNames] = useState<Record<string, string>>({});
  const [speakers, setSpeakers] = useState<SpeakerStatDto[]>([]);
  const [nextSpeakerId, setNextSpeakerId] = useState("speaker:1");
  const [mediaKind, setMediaKind] = useState<PlaybackMediaKind>("audio");
  const [sourceUrl, setSourceUrl] = useState<string | null>(null);
  const [hasMedia, setHasMedia] = useState(true);
  const [loadedNote, setLoadedNote] = useState<string | null>(null);
  /** 重试代数：自增触发聚合包 + 音源重载。 */
  const [reloadEpoch, setReloadEpoch] = useState(0);
  /** 事件处理器里取当前篇（它们挂一次、不随换篇重挂）。 */
  const noteRelRef = useRef<string | null>(noteRel);
  noteRelRef.current = noteRel;
  /** 待恢复的播放进度（自动重载时保留位置/播放态；`null` = 首载/显式重试从头开始）。 */
  const pendingRestoreRef = useRef<{ ms: number; playing: boolean } | null>(null);
  /** 上次写盘的播放位置（PRD-04-068 节流用）。 */
  const lastPersistRef = useRef<PersistMark | null>(null);

  const mediaRef = useRef<HTMLVideoElement>(null);
  const rafRef = useRef<number | null>(null);
  const cuesRef = useRef<CuePoint[]>([]);
  const playbackRateRef = useRef<PlaybackRate>(1);

  // rAF 同步：timeupdate (~4Hz) → 批量处理
  const pendingTimeRef = useRef<number | null>(null);

  useEffect(() => {
    cuesRef.current = cues;
  }, [cues]);

  // ── 加载聚合包并挂音源（一次性 IPC；composite 优先，单分片直链回退） ────────
  //
  // 音源走直链 Range 流式（绝不整段 blob，避免大音频 OOM，TDD-07 §3）。
  // 媒体元素 `crossorigin="anonymous"` 原生发 Range 请求；token 经 `?token=` 传递
  // （媒体元素无法设 Authorization 头）。

  useEffect(() => {
    let disposed = false;

    // 空载：卸掉音源并把状态收回初值。**不发 IPC**——宿主在用户打开任何一篇之前就已挂载。
    if (noteRel === null) {
      const media = mediaRef.current;
      if (media) {
        media.removeAttribute("src");
        media.load();
      }
      setCues([]);
      setBookmarks([]);
      setActiveCueIndex(-1);
      setPositionMs(0);
      setDurationMs(0);
      setError(null);
      setLoading(false);
      setPlaying(false);
      setLoadedNote(null);
      return;
    }

    setLoading(true);
    setError(null);
    setLoadedNote(null);
    // 换篇即复位播放态（真机 2026-09-08「没在放也显示暂停」）：媒体元素的 load 算法把
    // `paused` 置回 true 但**不发 `pause` 事件**，只听 play/pause 的话，上一篇放着时切到
    // 这一篇，`playing` 会一直挂着 true——图标显示「暂停」而什么都没在放。
    setPlaying(false);

    async function load() {
      const result = await commands.loadSyncPlayback(noteRel!);
      if (disposed) return;
      if (result.status === "error") {
        setError(vaultErrorText(result.error));
        setLoading(false);
        return;
      }
      const pack = result.data;
      setCues(normaliseCues(pack.cues));
      // specta f64 → number|null（NaN 序列化语义）：过滤 null 后升序；备注随行带上。
      setBookmarks(
        pack.bookmarks
          .flatMap((b) => (b.ms === null ? [] : [{ ms: b.ms, note: b.note }]))
          .sort((a, b) => a.ms - b.ms),
      );
      setDurationMs(pack.total_duration_ms ?? 0);
      setSpeakerNames(pack.speaker_names);
      setSpeakers(pack.speakers);
      setNextSpeakerId(pack.next_speaker_id);
      setMediaKind(pack.media_kind);
      setSourceUrl(pack.source_url);
      setHasMedia(pack.parts.length > 0);

      const url = pack.composite_id
        ? await vaultCompositeStreamUrl(pack.composite_id)
        : pack.fallback_media_rel
          ? await vaultMediaStreamUrl(pack.fallback_media_rel)
          : null;
      if (disposed) return;
      // 上次听到哪了（PRD-04-068）：只**定位**不起播——起播必须是显式意图（PRD-04-064 ⑦）。
      // 权威改写触发的重载已经带着更精确的 `pendingRestore`，那时不问。
      if (url !== null && pendingRestoreRef.current === null) {
        const saved = await commands.playbackPositionLoad(noteRel!);
        if (disposed) return;
        if (saved.status === "ok" && saved.data !== null && saved.data > 0) {
          pendingRestoreRef.current = { ms: saved.data, playing: false };
        }
      }
      lastPersistRef.current = null;
      if (url === null) {
        // 笔记无音频媒体：cue 列表仍可展示，仅禁用播放
        setLoadedNote(noteRel);
        setLoading(false);
        return;
      }
      const media = mediaRef.current;
      if (media) {
        media.src = url;
        media.playbackRate = playbackRateRef.current;
        // 自动重载（权威二次转写/细分改写落盘）恢复播放进度与播放态——避免用户正在
        // 听/看时被打断跳回开头（同一音频内容，绝对时间轴不变，PRD-04-030 无损刷新）。
        const restore = pendingRestoreRef.current;
        pendingRestoreRef.current = null;
        if (restore) {
          media.addEventListener(
            "loadedmetadata",
            () => {
              if (Number.isFinite(media.duration) && restore.ms > 0) {
                media.currentTime = Math.min(restore.ms / 1000, media.duration);
              }
              if (restore.playing) void media.play();
            },
            { once: true },
          );
        }
        media.load();
      }
      setLoadedNote(noteRel);
      setLoading(false);
    }

    void load().catch((err: unknown) => {
      if (!disposed) {
        setError(String(err));
        setLoading(false);
      }
    });

    return () => {
      disposed = true;
      const media = mediaRef.current;
      if (media) {
        persistPosition(noteRel, media);
        media.removeAttribute("src");
        media.load();
      }
    };
  }, [noteRel, reloadEpoch]);
  // 上面这段清理只在**换笔记 / 重载 / 宿主卸载**时跑。宿主随工作区常驻（不随空间切换
  // 卸载），因此"切到别的空间 → 音源被卸掉 → 播放中断"这条路已经不存在了（ADR-118）。

  // ── 权威改写落盘 → 位置无损重载（PRD-04-030 收口） ─────────────────────────
  //
  // 停录后台权威二次转写、手动二次转写、说话人细分完成后正文/边车被整体改写，均广播
  // `vault://changed`。回放视图据此**自动重载**聚合包与音源（cue/说话人徽标随之翻新），
  // 并保留当前播放进度/播放态（`pendingRestore`）。自写编辑（cue 编辑/改名）为 watcher
  // 抑制的静默写，不发此事件，故不会误触重载循环。
  useEffect(() => {
    if (noteRel === null) return;
    const unlisten = events.vaultChanged.listen((event) => {
      if (event.payload.rel_path !== noteRel || event.payload.deleted) return;
      const media = mediaRef.current;
      pendingRestoreRef.current = media
        ? { ms: media.currentTime * 1000, playing: !media.paused }
        : null;
      setReloadEpoch((epoch) => epoch + 1);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [noteRel]);

  // ── timeupdate → rAF 合并 → 二分查找 ────────────────────────────────────

  const flushPositionUpdate = useCallback(() => {
    rafRef.current = null;
    const ms = pendingTimeRef.current;
    if (ms === null) return;
    pendingTimeRef.current = null;
    setPositionMs(ms);
    const idx = findActiveCueIndex(cuesRef.current, ms);
    setActiveCueIndex(idx);
  }, []);

  useEffect(() => {
    const media = mediaRef.current;
    if (!media) return;

    function onTimeUpdate() {
      const media = mediaRef.current;
      if (!media) return;
      const ms = media.currentTime * 1000;
      pendingTimeRef.current = ms;
      // 记住听到哪了（PRD-04-068）：按节流写，暂停 / 换篇 / 播完另行即时写。
      const now = Date.now();
      if (!media.paused && shouldPersist(lastPersistRef.current, now, ms)) {
        lastPersistRef.current = { atMs: now, positionMs: ms };
        persistPosition(noteRelRef.current, media);
      }
      if (rafRef.current !== null) return;
      rafRef.current = window.requestAnimationFrame(flushPositionUpdate);
    }

    function onPlay() { setPlaying(true); }
    function onPause() {
      setPlaying(false);
      const media = mediaRef.current;
      if (media) persistPosition(noteRelRef.current, media);
    }
    function onEnded() {
      setPlaying(false);
      const media = mediaRef.current;
      if (media) persistPosition(noteRelRef.current, media);
    }
    // `emptied`：换源 / 卸源时 load 算法发的是它而不是 `pause`（见装载效果里的复位注释）。
    function onEmptied() { setPlaying(false); }
    function onLoadedMetadata() {
      // 聚合包未携带总时长（旧档/未知）时以解码时长兜底
      const media = mediaRef.current;
      if (media && Number.isFinite(media.duration)) {
        setDurationMs((prev) => (prev > 0 ? prev : media.duration * 1000));
      }
    }

    media.addEventListener("timeupdate", onTimeUpdate);
    media.addEventListener("play", onPlay);
    media.addEventListener("pause", onPause);
    media.addEventListener("ended", onEnded);
    media.addEventListener("emptied", onEmptied);
    media.addEventListener("loadedmetadata", onLoadedMetadata);

    return () => {
      media.removeEventListener("timeupdate", onTimeUpdate);
      media.removeEventListener("play", onPlay);
      media.removeEventListener("pause", onPause);
      media.removeEventListener("ended", onEnded);
      media.removeEventListener("emptied", onEmptied);
      media.removeEventListener("loadedmetadata", onLoadedMetadata);
      if (rafRef.current !== null) {
        window.cancelAnimationFrame(rafRef.current);
        rafRef.current = null;
      }
    };
  }, [flushPositionUpdate]);

  // ── Actions ──────────────────────────────────────────────────────────────

  const seekToMs = useCallback((ms: number) => {
    const media = mediaRef.current;
    if (!media) return;
    media.currentTime = ms / 1000;
    dispatchFollow({ type: "seek_to_cue" });
  }, []);

  const seekToCue = useCallback((index: number) => {
    const cue = cuesRef.current[index];
    if (!cue) return;
    seekToMs(cue.start_ms);
  }, [seekToMs]);

  const togglePlay = useCallback(() => {
    const media = mediaRef.current;
    if (!media) return;
    if (media.paused) {
      void media.play();
    } else {
      media.pause();
    }
  }, []);

  const returnToCurrent = useCallback(() => {
    dispatchFollow({ type: "return_to_current" });
    const idx = activeCueIndex >= 0 ? activeCueIndex : 0;
    seekToCue(idx);
  }, [activeCueIndex, seekToCue]);

  const setPlaybackRate = useCallback((rate: PlaybackRate) => {
    playbackRateRef.current = rate;
    setPlaybackRateState(rate);
    const media = mediaRef.current;
    if (media) media.playbackRate = rate;
  }, []);

  const retry = useCallback(() => {
    setReloadEpoch((epoch) => epoch + 1);
  }, []);

  const onUserScroll = useCallback(() => {
    dispatchFollow({ type: "user_scroll" });
  }, []);

  const renameSpeaker = useCallback(
    async (speaker: string, name: string): Promise<string | null> => {
      if (noteRel === null) return null;
      const result = await commands.setSpeakerName(noteRel, speaker, name);
      if (result.status === "error") {
        return result.error;
      }
      const trimmed = name.trim();
      setSpeakerNames((prev) => {
        const next = { ...prev };
        if (trimmed) {
          next[speaker] = trimmed;
        } else {
          delete next[speaker];
        }
        return next;
      });
      return null;
    },
    [noteRel],
  );

  const editSegment = useCallback(
    async (edit: SegmentEditDto): Promise<string | null> => {
      if (noteRel === null) return null;
      const result = await commands.editPlaybackSegment(noteRel, edit);
      if (result.status === "error") return result.error;
      // 段界/归属已在 Rust 权威落盘（静默自写不发 vault://changed）→ 主动位置无损重载。
      const media = mediaRef.current;
      pendingRestoreRef.current = media
        ? { ms: media.currentTime * 1000, playing: !media.paused }
        : null;
      setReloadEpoch((epoch) => epoch + 1);
      return null;
    },
    [noteRel],
  );

  return {
    playbackState: {
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
      speakerNames,
      speakers,
      nextSpeakerId,
      mediaKind,
      sourceUrl,
      hasMedia,
      loadedNote,
    },
    actions: {
      seekToMs,
      seekToCue,
      togglePlay,
      returnToCurrent,
      onUserScroll,
      setBilingualMode,
      setPlaybackRate,
      retry,
      renameSpeaker,
      editSegment,
    },
    mediaRef,
  };
}

// ─── 导出纯逻辑（供测试）────────────────────────────────────────────────────

export { findActiveCueIndex, followReducer, keyboardSeekMs } from "../recording/recording-cue";
