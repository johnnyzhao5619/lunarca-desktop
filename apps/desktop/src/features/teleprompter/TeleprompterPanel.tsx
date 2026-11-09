/**
 * 提词器面板（ADR-162 ⑤ C4a，PRD-19-016）：常驻浮动窗 `teleprompter`。
 *
 * 显示 Rust 从脚本抽出的要念的文字（`teleprompter_content`）；字号、行宽、镜像、速度来自内部
 * 设置（`use-teleprompter-prefs`）。面板不取键盘焦点（录屏 / 录像软件留在前台），控制全靠可点
 * 的按钮；匀速滚动只是呈现动画——按设定速度推进滚动位置，内容判定一律在 Rust。
 *
 * 跟读（C4b，PRD-19-015）：对齐在 Rust（`teleprompter_follow_start`），念到的位置经二进制
 * Channel 送来（段、行、字，各 4 字节 LE）；面板只把那一行带到阅读线上并标出来。
 *
 * 录制标记（C4c，PRD-19-017）：「标记重来」把此刻那一行报给 Rust（跟读时是高亮行，没跟读时是
 * 阅读线上的那一行——哪一行在阅读线上是呈现几何，归面板）；标记攒在 Rust，停止跟读或收起时
 * 由 Rust 写回脚本。快捷键路径（录音打点键）不经面板，Rust 经 `__lunaTeleprompterMarks` 告知计数。
 *
 * 录制页签的「开始跟读」（ADR-165 ⑧）：`teleprompter_open(path, true)` 显示面板后经
 * `__lunaTeleprompterFollow` 请面板开跟读——会话通道在面板里，只能由面板自己开。
 */

import { Fragment, useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Channel } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import {
  commands,
  events,
  type RawBytes,
  type PrompterAnchorDto,
  type PrompterSpanDto,
  type RecordingEvent,
  type TeleprompterDto,
} from "../../ipc/bindings";
import { CloseIcon, MicIcon, MinusIcon, PauseIcon, PlayIcon, PlusIcon } from "@lunarca/editor-core/Icon";
import { HudActionMenu, type HudActionMenuItem } from "../../components/ui/HudActionMenu";
import { IconButton } from "../../components/ui/IconButton";
import { useWindowDrag } from "../../hooks/use-window-drag";
import { cssVariables } from "../../lib/css-variables";
import { formatClock } from "../../lib/format-duration";
import { vaultErrorText } from "../../lib/vault-error";
import { importTeleprompterFiles } from "./teleprompter-actions";
import { useTeleprompterPrefs, type NumberPref, type TeleprompterNumberKey } from "./use-teleprompter-prefs";

/** 阅读线：阅读面上方三分之一处（跟读把当前行带到这里，没跟读时标记取这里的那一行）。 */
const READING_LINE = 1 / 3;

/** 跟读位置（段、行；与 `content.blocks` / `lines` 的下标同一套）。 */
interface ReadAt {
  block: number;
  line: number;
}

/** 从这一行（含）到稿末约还要念多少毫秒（Rust 按本人语速给了每行的毫秒数，这里只累加）。 */
export function remainingMillis(content: TeleprompterDto, from: ReadAt): number {
  let total = 0;
  content.blocks.forEach((block, blockIndex) => {
    block.lines.forEach((line, lineIndex) => {
      if (blockIndex > from.block || (blockIndex === from.block && lineIndex >= from.line)) total += line.millis;
    });
  });
  return total;
}

/** 浮窗上全部的行，按念的顺序。 */
function lineElements(box: HTMLElement): HTMLElement[] {
  return [...box.querySelectorAll<HTMLElement>("[data-at]")];
}

function atOf(line: HTMLElement): ReadAt {
  const [block, index] = (line.dataset.at ?? "").split("-").map(Number);
  return { block, line: index };
}

/**
 * 从 `from` 起往前（-1）或往后（+1）挪一行或一段，返回目标行；挪不动（已在头 / 尾）返回 `null`。
 * 换段 = 落到相邻一段的第一行。纯呈现计算：行就是浮窗上画出来的那些。
 */
export function neighborLine(
  box: HTMLElement,
  from: ReadAt,
  step: 1 | -1,
  unit: "line" | "block",
): ReadAt | null {
  const lines = lineElements(box).map(atOf);
  const index = lines.findIndex((at) => at.block === from.block && at.line === from.line);
  if (index === -1) return lines[0] ?? null;
  if (unit === "line") return lines[index + step] ?? null;
  if (step === 1) return lines.find((at) => at.block > from.block) ?? null;
  // 往前换段：在本段中间就回到本段第一行，已在本段第一行就到上一段第一行。
  const target = from.line > 0 ? from.block : from.block - 1;
  return lines.find((at) => at.block === target) ?? null;
}

/** 解一帧跟读位置（Rust 发的 12 字节：段、行、字，各 u32 LE；字这里用不上）。 */
export function decodeReadAt(frame: ArrayBuffer | ArrayLike<number>): ReadAt | null {
  const bytes = frame instanceof ArrayBuffer ? new Uint8Array(frame) : Uint8Array.from(frame);
  if (bytes.length < 8) return null;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  return { block: view.getUint32(0, true), line: view.getUint32(4, true) };
}

/**
 * 一行在滚动区内容里的纵向位置（px，从内容顶端算）。不用 `offsetTop`：浮窗根元素是定位元素，
 * `offsetTop` 是相对整个浮窗算的，会多出控制条那一截（窄条里控制条折成两行，约 80px），带到阅读线
 * 的行就会偏到阅读线上方、被切掉半截（真机 2026-11-04）。
 */
export function lineTopInBox(line: HTMLElement, box: HTMLElement): number {
  return line.getBoundingClientRect().top - box.getBoundingClientRect().top + box.scrollTop;
}

/** 判定「在阅读线上」时的容差（px）：平滑滚动停下的位置会有一两个像素的取整误差。 */
const READING_LINE_SLACK = 2;

/** 阅读线上（或刚越过阅读线）的那一行；一行都没有返回 `null`。 */
export function lineAtReadingLine(box: HTMLElement): ReadAt | null {
  const reading = box.scrollTop + box.clientHeight * READING_LINE + READING_LINE_SLACK;
  let found: ReadAt | null = null;
  for (const line of box.querySelectorAll<HTMLElement>("[data-at]")) {
    const [block, index] = (line.dataset.at ?? "").split("-").map(Number);
    if (found && lineTopInBox(line, box) > reading) break;
    found = { block, line: index };
  }
  return found;
}

declare global {
  interface Window {
    /** Rust 换脚本后请常驻面板重读（`show_teleprompter` 经 eval 调用）。 */
    __lunaReloadTeleprompter?: () => void;
    /** Rust 告知录制标记：此刻攒了几次、刚写回了几处（快捷键打标记、写回之后经 eval 调用）。 */
    __lunaTeleprompterMarks?: (pending: number, written: number) => void;
    /** Rust 请面板开始跟读（录制页签「开始跟读」，`teleprompter_open` 经 eval 调用）；已开着则不动。 */
    __lunaTeleprompterFollow?: () => void;
    /** Rust 转交浮窗按键（ADR-176 ⑥：浮窗显示时注册的全局键与遥控键），参数是动作名。 */
    __lunaTeleprompterKey?: (key: string) => void;
  }
}

/** 一行里的一段口播记号（ADR-176 ⑦）：怎么拆归 Rust，这里只按种类画。 */
function PrompterSpan({ span, pauseLabel }: { span: PrompterSpanDto; pauseLabel: string }) {
  switch (span.kind) {
    case "text":
      return <Fragment>{span.text}</Fragment>;
    case "emphasis":
      return <strong className="luna-teleprompter__em">{span.text}</strong>;
    case "pause":
      return <span className="luna-teleprompter__pause" role="img" aria-label={pauseLabel} />;
    case "note":
      return <span className="luna-teleprompter__note">{span.text}</span>;
  }
}

export function TeleprompterPanel() {
  const { t } = useTranslation("workspace");
  const prefs = useTeleprompterPrefs();
  const { onPointerDown } = useWindowDrag();
  const [content, setContent] = useState<TeleprompterDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [playing, setPlaying] = useState(false);
  /** 跟读：关 / 正在开 / 开着。 */
  const [follow, setFollow] = useState<"off" | "starting" | "on">("off");
  const [readAt, setReadAt] = useState<ReadAt | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  /** 攒着、还没写回脚本的录制标记次数。 */
  const [pendingMarks, setPendingMarks] = useState(0);
  const followSessionRef = useRef<string | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const speedRef = useRef(prefs.speed.value);
  speedRef.current = prefs.speed.value;
  /** 开始前倒计时（ADR-176 ⑤）：还剩几秒；倒计时结束才开始滚动或跟读（不提前碰麦克风）。 */
  const [countdown, setCountdown] = useState<{ left: number; then: "play" | "follow" } | null>(null);
  /** 计时（ADR-176 ⑤）：已用时间 = 之前攒下的 + 这一轮开始以来的；每秒刷新一次显示。 */
  const elapsedRef = useRef({ carried: 0, since: null as number | null });
  const [clockTick, setClockTick] = useState(0);
  const extended = content?.extended ?? false;
  const extendedRef = useRef(extended);
  extendedRef.current = extended;
  /** 有文件正拖在浮窗上（ADR-176 ⑨，PRD-20-022）。 */
  const [dragging, setDragging] = useState(false);
  /**
   * 上一次按键 / 点击带到阅读线的是哪一行。下一次换行换段从它挪，不再从屏幕几何反推——平滑滚动
   * 停下的位置有几个像素的偏差，落在两行分界上就会判成上一行，按键就原地不动（真机 2026-11-04）。
   * 手动滚动、匀速滚动、换稿、回到开头之后作废，回到几何判定。
   */
  const navRef = useRef<ReadAt | null>(null);

  // 拖文件到浮窗：转成提词稿并换成那一篇（转换、建稿、打开都在 Rust；浮窗随后被请去重读）。
  // 这是提词器应用的扩展：只装创作时浮窗不接文件。
  useEffect(() => {
    let disposed = false;
    const pending = getCurrentWebview().onDragDropEvent((event) => {
      if (disposed || !extendedRef.current) return;
      const kind = event.payload.type;
      if (kind === "enter" || kind === "over") {
        setDragging(true);
        return;
      }
      setDragging(false);
      if (kind !== "drop") return;
      void importTeleprompterFiles(event.payload.paths).then((failure) => {
        if (failure) setNotice(failure);
      });
    });
    return () => {
      disposed = true;
      void pending.then((dispose) => dispose());
    };
  }, []);

  /** 上一次显示的是哪一篇（换篇才回到开头）。 */
  const shownPathRef = useRef<string | null>(null);

  /**
   * 改稿重读（PRD-20-015）：带上「刚才那一行」——段号行号与要念的文字——请 Rust 在新稿里找回它；
   * 跟读开着时 Rust 同时用新稿重建对齐器、从那一行接着跟。
   */
  const contentRef = useRef(content);
  contentRef.current = content;
  const readAtRef = useRef(readAt);
  readAtRef.current = readAt;
  /** 重读回来、要在新稿上带到阅读线的那一行（等新稿画出来再跳）。 */
  const pendingAnchorRef = useRef<ReadAt | null>(null);

  const load = useCallback(async () => {
    const shown = contentRef.current;
    const box = scrollRef.current;
    let anchor: PrompterAnchorDto | null = null;
    if (shown?.path && box) {
      const at = readAtRef.current ?? navRef.current ?? lineAtReadingLine(box);
      const text = at ? shown.blocks[at.block]?.lines[at.line]?.text : undefined;
      if (at && text !== undefined) anchor = { path: shown.path, block: at.block, line: at.line, text };
    }
    const reply = await commands.teleprompterContent(anchor);
    if (reply.status !== "ok") {
      setError(vaultErrorText(reply.error));
      return;
    }
    setError(null);
    // 换了一篇脚本：从头开始、停下滚动；同一篇被保存：保持位置。
    if (shownPathRef.current !== reply.data.path) {
      shownPathRef.current = reply.data.path;
      setPlaying(false);
      setCountdown(null);
      elapsedRef.current = { carried: 0, since: null };
      scrollRef.current?.scrollTo({ top: 0 });
    } else if (reply.data.anchor) {
      pendingAnchorRef.current = reply.data.anchor;
    }
    setContent(reply.data);
  }, []);

  useEffect(() => {
    void load();
    // 重新打开：上一回的提示（例如「已写回」）已经过时。
    window.__lunaReloadTeleprompter = () => {
      setNotice(null);
      void load();
    };
    window.__lunaTeleprompterMarks = (pending, written) => {
      setPendingMarks(pending);
      if (written > 0) setNotice(t("teleprompter.panel.marksWritten", { count: written }));
    };
    return () => {
      delete window.__lunaReloadTeleprompter;
      delete window.__lunaTeleprompterMarks;
    };
  }, [load, t]);

  // 脚本在编辑器里被保存：跟着更新文字（不跳位置）。
  const path = content?.path ?? null;
  useEffect(() => {
    if (!path) return;
    const unlisten = events.noteWritten.listen((event) => {
      if (event.payload.path === path) void load();
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [path, load]);

  // 匀速滚动：按设定速度（px/s）推进滚动位置；滚到底自动停。
  useEffect(() => {
    if (!playing) return;
    let frame = 0;
    let last = performance.now();
    let carry = 0;
    const tick = (now: number) => {
      const box = scrollRef.current;
      if (!box) return;
      carry += ((now - last) / 1000) * speedRef.current;
      last = now;
      const whole = Math.floor(carry);
      if (whole > 0) {
        box.scrollTop += whole;
        carry -= whole;
      }
      if (box.scrollTop + box.clientHeight >= box.scrollHeight - 1) {
        setPlaying(false);
        return;
      }
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [playing]);

  // 计时：滚动或跟读进行中算「在念」；停下时把这一轮攒进去。
  const running = playing || follow === "on";
  useEffect(() => {
    const clock = elapsedRef.current;
    if (running) {
      clock.since ??= performance.now();
      const timer = window.setInterval(() => setClockTick((n) => n + 1), 1000);
      return () => window.clearInterval(timer);
    }
    if (clock.since !== null) {
      clock.carried += performance.now() - clock.since;
      clock.since = null;
    }
    return undefined;
  }, [running]);

  // 跟读送来新位置：把那一行带到阅读线（阅读面上方三分之一处）。
  useEffect(() => {
    const box = scrollRef.current;
    if (!readAt || !box) return;
    const line = box.querySelector<HTMLElement>(`[data-at="${readAt.block}-${readAt.line}"]`);
    if (!line) return;
    box.scrollTo({ top: lineTopInBox(line, box) - box.clientHeight * READING_LINE, behavior: "smooth" });
  }, [readAt]);

  const stopFollow = useCallback(async () => {
    const session = followSessionRef.current;
    followSessionRef.current = null;
    setFollow("off");
    setReadAt(null);
    if (session) await commands.recordingStop(session);
  }, []);

  const startFollow = async () => {
    setPlaying(false);
    setNotice(null);
    setFollow("starting");
    const session = crypto.randomUUID();
    const sessionEvents = new Channel<RecordingEvent>();
    sessionEvents.onmessage = (event) => {
      // 会话自己结束了（出错、麦克风没了）：开关跟着复位，说清原因。
      if (event.kind === "failed") {
        setNotice(vaultErrorText(event.data.code));
        followSessionRef.current = null;
        setFollow("off");
      } else if (event.kind === "state" && event.data.status === "idle") {
        followSessionRef.current = null;
        setFollow("off");
      }
    };
    const positions = new Channel<ArrayBuffer>();
    positions.onmessage = (frame) => {
      const at = decodeReadAt(frame);
      if (at) setReadAt(at);
    };
    const reply = await commands.teleprompterFollowStart(
      session,
      sessionEvents,
      positions as unknown as Channel<RawBytes>,
    );
    if (reply.status !== "ok") {
      setFollow("off");
      setNotice(vaultErrorText(reply.error));
      return;
    }
    // 麦克风被录音 / 字幕 / 语音输入占着时，命令照样返回，但状态不是「录音中」：没开起来。
    if (reply.data.status !== "recording") {
      setFollow("off");
      setNotice(t(reply.data.message_key ?? "teleprompter.panel.followFailed"));
      return;
    }
    followSessionRef.current = session;
    setFollow("on");
  };

  // 录制页签「开始跟读」：Rust 打开面板后请它开跟读（已在开或开着就不动）。
  const followRef = useRef(follow);
  followRef.current = follow;
  const startFollowRef = useRef(startFollow);
  startFollowRef.current = startFollow;
  // 倒计时一秒一跳，到 0 才开始。
  useEffect(() => {
    if (!countdown) return;
    if (countdown.left <= 0) {
      setCountdown(null);
      if (countdown.then === "play") setPlaying(true);
      else void startFollowRef.current();
      return;
    }
    const timer = window.setTimeout(
      () => setCountdown((current) => (current ? { ...current, left: current.left - 1 } : null)),
      1000,
    );
    return () => window.clearTimeout(timer);
  }, [countdown]);

  /** 开始滚动或跟读：装了「提词器」且设了倒计时就先倒数（ADR-176 ⑤），否则立刻开始。 */
  const begin = (then: "play" | "follow") => {
    if (extended && prefs.countdownSecs > 0) {
      setCountdown({ left: prefs.countdownSecs, then });
      return;
    }
    if (then === "play") setPlaying(true);
    else void startFollowRef.current();
  };

  // 跟读开关被关掉（首页或别处）：正在跟读的会话随之停下。
  const stopFollowRef = useRef(stopFollow);
  stopFollowRef.current = stopFollow;
  useEffect(() => {
    if (!prefs.followEnabled && followRef.current === "on") void stopFollowRef.current();
  }, [prefs.followEnabled]);
  const beginRef = useRef(begin);
  beginRef.current = begin;
  useEffect(() => {
    window.__lunaTeleprompterFollow = () => {
      if (followRef.current === "off") beginRef.current("follow");
    };
    return () => {
      delete window.__lunaTeleprompterFollow;
    };
  }, []);

  const mark = async () => {
    const box = scrollRef.current;
    // 跟读时是念到的那一行；按键 / 点行 / 改稿找回之后是带到阅读线的那一行；否则看阅读线。
    const at = readAt ?? navRef.current ?? (box ? lineAtReadingLine(box) : null);
    if (!at) return;
    const reply = await commands.teleprompterMark(at);
    if (reply.status !== "ok") {
      setNotice(vaultErrorText(reply.error));
      return;
    }
    setPendingMarks(reply.data);
  };

  /** 开始 / 暂停匀速滚动（跟读开着时滚动由跟读带，不响应）。 */
  const togglePlay = () => {
    if (!content?.path || follow !== "off") return;
    if (playing || countdown) {
      setPlaying(false);
      setCountdown(null);
    } else {
      begin("play");
    }
  };

  const goTop = () => {
    navRef.current = null;
    scrollRef.current?.scrollTo({ top: 0 });
    elapsedRef.current = { carried: 0, since: running ? performance.now() : null };
    setClockTick((n) => n + 1);
    if (follow === "on") {
      setReadAt(null);
      void commands.teleprompterFollowReset();
    }
  };

  const closePanel = () => {
    setPlaying(false);
    setCountdown(null);
    // 收起即停跟读：面板看不见了，不该还在听麦克风。
    void stopFollow().then(() => commands.teleprompterHide());
  };


  /** 把某一行带到阅读线（PRD-20-013）；跟读开着时 Rust 也从这一行接着跟。 */
  const jumpTo = (at: ReadAt) => {
    const box = scrollRef.current;
    const line = box?.querySelector<HTMLElement>(`[data-at="${at.block}-${at.line}"]`);
    if (!box || !line) return;
    navRef.current = at;
    box.scrollTo({ top: lineTopInBox(line, box) - box.clientHeight * READING_LINE, behavior: "smooth" });
    if (follow === "on") {
      setReadAt(at);
      void commands.teleprompterFollowSeek(at);
    }
  };

  /** 换行 / 换段：从此刻那一行（跟读时是念到的那一行）挪。 */
  const move = (step: 1 | -1, unit: "line" | "block") => {
    const box = scrollRef.current;
    if (!box) return;
    const from = readAt ?? navRef.current ?? lineAtReadingLine(box);
    const target = from ? neighborLine(box, from, step, unit) : null;
    if (target) jumpTo(target);
  };

  // 浮窗按键（ADR-176 ⑥）：Rust 注册、转交动作名，这里执行。
  const keyActions: Record<string, () => void> = {
    play: togglePlay,
    line_up: () => move(-1, "line"),
    line_down: () => move(1, "line"),
    block_prev: () => move(-1, "block"),
    block_next: () => move(1, "block"),
    faster: () => prefs.stepNumber("teleprompter.speed", 1),
    slower: () => prefs.stepNumber("teleprompter.speed", -1),
    current: () => {
      if (readAt) jumpTo(readAt);
    },
    top: goTop,
    close: closePanel,
    mark: () => void mark(),
  };
  const keyActionsRef = useRef(keyActions);
  keyActionsRef.current = keyActions;
  useEffect(() => {
    window.__lunaTeleprompterKey = (key) => keyActionsRef.current[key]?.();
    return () => {
      delete window.__lunaTeleprompterKey;
    };
  }, []);

  // 点哪一行从哪一行开始（PRD-20-013）：只认点在行上的那一下。
  const jumpToRef = useRef(jumpTo);
  jumpToRef.current = jumpTo;
  // 改稿重读回来：新稿画出来之后把找回的那一行带到阅读线（跟读开着时 Rust 已从那一行接着跟）。
  useEffect(() => {
    const at = pendingAnchorRef.current;
    if (!at) return;
    pendingAnchorRef.current = null;
    jumpToRef.current(at);
  }, [content]);
  useEffect(() => {
    const box = scrollRef.current;
    if (!box) return;
    const onClick = (event: MouseEvent) => {
      const line = (event.target as Element | null)?.closest<HTMLElement>("[data-at]");
      if (line) jumpToRef.current(atOf(line));
    };
    // 手动滚过：按键导航改回从阅读线上的那一行算。
    const onWheel = () => {
      navRef.current = null;
    };
    box.addEventListener("click", onClick);
    box.addEventListener("wheel", onWheel, { passive: true });
    return () => {
      box.removeEventListener("click", onClick);
      box.removeEventListener("wheel", onWheel);
    };
  }, []);
  // 匀速滚动或换了一篇：之前记下的那一行作废。
  useEffect(() => {
    if (playing) navRef.current = null;
  }, [playing]);
  useEffect(() => {
    navRef.current = null;
  }, [content?.path]);

  const stepper = (key: TeleprompterNumberKey, label: string, pref: NumberPref) => (
    <div className="luna-teleprompter__stepper" role="group" aria-label={label}>
      <IconButton
        density="sm"
        label={t("teleprompter.panel.less", { what: label })}
        icon={<MinusIcon size={12} />}
        disabled={pref.value <= pref.min}
        onClick={() => prefs.stepNumber(key, -1)}
      />
      <span className="luna-teleprompter__value">{label} {pref.value}</span>
      <IconButton
        density="sm"
        label={t("teleprompter.panel.more", { what: label })}
        icon={<PlusIcon size={12} />}
        disabled={pref.value >= pref.max}
        onClick={() => prefs.stepNumber(key, 1)}
      />
    </div>
  );

  // 已用 / 预计剩余（只在装了「提词器」时显示）。`clockTick` 驱动每秒重算。
  void clockTick;
  const clock = elapsedRef.current;
  const elapsedMs = clock.carried + (clock.since === null ? 0 : performance.now() - clock.since);
  const box = scrollRef.current;
  // 与换行、标记同一个「此刻那一行」：平滑滚动还没停下时阅读线几何会差一行（真机 2026-11-04）。
  const at = readAt ?? navRef.current ?? (box ? lineAtReadingLine(box) : null) ?? { block: 0, line: 0 };
  const clockText = content?.path
    ? t("teleprompter.panel.clock", {
        elapsed: formatClock(elapsedMs),
        remaining: formatClock(remainingMillis(content, at)),
      })
    : "";

  /** 「···」菜单（ADR-176 ④ ⑤）：跟读开关两个应用都有；其余是提词器应用的扩展。 */
  const menuItems: HudActionMenuItem[] = [
    {
      id: "follow",
      label: t("teleprompter.panel.menu.follow"),
      checked: prefs.followEnabled,
      onSelect: () => void prefs.setFollowEnabled(!prefs.followEnabled),
    },
    ...(extended
      ? [
          {
            id: "camera",
            label: t("teleprompter.panel.menu.camera"),
            onSelect: () => {
              void commands.teleprompterSnapToCamera().then((reply) => {
                if (reply.status !== "ok") setNotice(vaultErrorText(reply.error));
              });
            },
          },
          {
            id: "remote",
            label: t("teleprompter.panel.menu.remote"),
            checked: prefs.remoteEnabled,
            onSelect: () => prefs.setRemoteEnabled(!prefs.remoteEnabled),
          },
          {
            id: "capture",
            label: t("teleprompter.panel.menu.captureHidden"),
            checked: prefs.captureHidden,
            onSelect: () => prefs.setCaptureHidden(!prefs.captureHidden),
          },
          ...prefs.countdownOptions.map((secs) => ({
            id: `countdown-${secs}`,
            label:
              secs === 0
                ? t("teleprompter.panel.menu.countdownOff")
                : t("teleprompter.panel.menu.countdown", { secs }),
            checked: prefs.countdownSecs === secs,
            onSelect: () => prefs.setCountdownSecs(secs),
          })),
        ]
      : []),
  ];

  return (
    <main className="luna-teleprompter" data-mirror={prefs.mirror || undefined} data-dragging={dragging || undefined}>
      <header className="luna-teleprompter__bar" onPointerDown={onPointerDown}>
        <span className="luna-teleprompter__title">{content?.title || t("teleprompter.panel.title")}</span>
        {/* 跟读由用户开、默认关（ADR-176 ④）：关着时面板上没有跟读控件。 */}
        {prefs.followEnabled && (
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-btn--sm"
            aria-pressed={follow === "on"}
            disabled={!content?.path || follow === "starting" || countdown !== null}
            onClick={() => (follow === "on" ? void stopFollow() : begin("follow"))}
          >
            <MicIcon size={12} />
            {t(follow === "on" ? "teleprompter.panel.followStop" : "teleprompter.panel.follow")}
          </button>
        )}
        {/* 跟读只用本机识别（ADR-176 ④）：跟读进行时写明，声音不出本机。 */}
        {follow === "on" ? (
          <span className="luna-teleprompter__badge" title={t("teleprompter.panel.followLocalHint")}>
            {t("teleprompter.panel.followLocal")}
          </span>
        ) : null}
        <button
          type="button"
          className="luna-btn luna-btn--primary luna-btn--sm"
          disabled={!content?.path || follow !== "off"}
          onClick={togglePlay}
        >
          {playing || countdown ? <PauseIcon size={12} /> : <PlayIcon size={12} />}
          {t(playing || countdown ? "teleprompter.panel.pause" : "teleprompter.panel.play")}
        </button>
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          title={t("teleprompter.panel.markHint")}
          disabled={!content?.path}
          onClick={() => void mark()}
        >
          {t("teleprompter.panel.mark")}
        </button>
        {stepper("teleprompter.speed", t("teleprompter.panel.speed"), prefs.speed)}
        {stepper("teleprompter.font_size", t("teleprompter.panel.fontSize"), prefs.fontSize)}
        {stepper("teleprompter.line_width", t("teleprompter.panel.lineWidth"), prefs.lineWidth)}
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          aria-pressed={prefs.mirror}
          onClick={() => prefs.setMirror(!prefs.mirror)}
        >
          {t("teleprompter.panel.mirror")}
        </button>
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          onClick={goTop}
        >
          {t("teleprompter.panel.top")}
        </button>
        {/* 遥控模式开着时常亮（ADR-176 ⑥）：那几个不带修饰的键此刻归提词器。 */}
        {extended && prefs.remoteEnabled ? (
          <span className="luna-teleprompter__badge" role="status" title={t("teleprompter.panel.remoteHint")}>
            {t("teleprompter.panel.remoteOn")}
          </span>
        ) : null}
        {extended && content?.path ? (
          <span className="luna-teleprompter__clock" role="timer" aria-label={t("teleprompter.panel.clockLabel")}>
            {clockText}
          </span>
        ) : null}
        <HudActionMenu label={t("teleprompter.panel.more_menu")} items={menuItems} align="end" side="bottom" density="sm" />
        <IconButton
          label={t("teleprompter.panel.close")}
          icon={<CloseIcon size={14} />}
          onClick={closePanel}
        />
      </header>
      <div
        ref={scrollRef}
        className="luna-teleprompter__scroll"
        style={cssVariables({
          "--prompter-font-size": `${prefs.fontSize.value}px`,
          "--prompter-line-width": `${prefs.lineWidth.value}%`,
        })}
      >
        {/* 攒了几次标记写在提示行里而不是按钮上：按钮宽度一变整条工具栏会重排，下一次点击就点偏了。 */}
        {countdown ? (
          <p className="luna-teleprompter__countdown" role="status" aria-live="assertive">
            {countdown.left}
          </p>
        ) : null}
        {notice || pendingMarks > 0 ? (
          <p className="luna-teleprompter__notice" role="status">
            {notice ?? t("teleprompter.panel.markPending", { count: pendingMarks })}
          </p>
        ) : null}
        {error ? (
          <p className="luna-teleprompter__empty" role="status">
            {error}
          </p>
        ) : !content?.path ? (
          <p className="luna-teleprompter__empty">{t("teleprompter.panel.empty")}</p>
        ) : (
          <div className="luna-teleprompter__text">
            {content.blocks.map((block, blockIndex) => (
              <section key={`${blockIndex}-${block.heading ?? ""}`} className="luna-teleprompter__block">
                {block.heading ? <h2 className="luna-teleprompter__heading">{block.heading}</h2> : null}
                {block.lines.map((line, lineIndex) => (
                  <p
                    key={lineIndex}
                    className={line.cue ? "luna-teleprompter__cue" : "luna-teleprompter__line"}
                    data-at={`${blockIndex}-${lineIndex}`}
                    data-current={(readAt?.block === blockIndex && readAt.line === lineIndex) || undefined}
                  >
                    {line.marker ? <span className="luna-teleprompter__marker">{line.marker}</span> : null}
                    {line.spans.map((span, spanIndex) => (
                      <PrompterSpan key={spanIndex} span={span} pauseLabel={t("teleprompter.panel.pauseMark")} />
                    ))}
                  </p>
                ))}
              </section>
            ))}
          </div>
        )}
      </div>
      {/* 无边框窗的系统缩放边只有一两个像素宽：给一个看得见、抓得住的角（同桌面便签）。 */}
      <button
        type="button"
        className="luna-teleprompter__resize"
        aria-label={t("teleprompter.panel.resize")}
        title={t("teleprompter.panel.resize")}
        onPointerDown={(event) => {
          event.preventDefault();
          void getCurrentWebviewWindow().startResizeDragging("SouthEast");
        }}
      />
    </main>
  );
}
