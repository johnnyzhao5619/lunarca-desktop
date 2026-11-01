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

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Channel } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import {
  commands,
  events,
  type RawBytes,
  type RecordingEvent,
  type TeleprompterDto,
} from "../../ipc/bindings";
import { CloseIcon, MicIcon, MinusIcon, PauseIcon, PlayIcon, PlusIcon } from "../../components/ui/Icon";
import { IconButton } from "../../components/ui/IconButton";
import { useWindowDrag } from "../../hooks/use-window-drag";
import { cssVariables } from "../../lib/css-variables";
import { vaultErrorText } from "../../lib/vault-error";
import { useTeleprompterPrefs, type NumberPref, type TeleprompterNumberKey } from "./use-teleprompter-prefs";

/** 阅读线：阅读面上方三分之一处（跟读把当前行带到这里，没跟读时标记取这里的那一行）。 */
const READING_LINE = 1 / 3;

/** 跟读位置（段、行；与 `content.blocks` / `lines` 的下标同一套）。 */
interface ReadAt {
  block: number;
  line: number;
}

/** 解一帧跟读位置（Rust 发的 12 字节：段、行、字，各 u32 LE；字这里用不上）。 */
export function decodeReadAt(frame: ArrayBuffer | ArrayLike<number>): ReadAt | null {
  const bytes = frame instanceof ArrayBuffer ? new Uint8Array(frame) : Uint8Array.from(frame);
  if (bytes.length < 8) return null;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  return { block: view.getUint32(0, true), line: view.getUint32(4, true) };
}

/** 阅读线上（或刚越过阅读线）的那一行；一行都没有返回 `null`。 */
export function lineAtReadingLine(box: HTMLElement): ReadAt | null {
  const reading = box.scrollTop + box.clientHeight * READING_LINE;
  let found: ReadAt | null = null;
  for (const line of box.querySelectorAll<HTMLElement>("[data-at]")) {
    const [block, index] = (line.dataset.at ?? "").split("-").map(Number);
    if (found && line.offsetTop > reading) break;
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

  /** 上一次显示的是哪一篇（换篇才回到开头）。 */
  const shownPathRef = useRef<string | null>(null);

  const load = useCallback(async () => {
    const reply = await commands.teleprompterContent();
    if (reply.status !== "ok") {
      setError(vaultErrorText(reply.error));
      return;
    }
    setError(null);
    // 换了一篇脚本：从头开始、停下滚动；同一篇被保存：保持位置。
    if (shownPathRef.current !== reply.data.path) {
      shownPathRef.current = reply.data.path;
      setPlaying(false);
      scrollRef.current?.scrollTo({ top: 0 });
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
      if (written > 0) setNotice(t("creator.teleprompter.marksWritten", { count: written }));
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

  // 跟读送来新位置：把那一行带到阅读线（阅读面上方三分之一处）。
  useEffect(() => {
    const box = scrollRef.current;
    if (!readAt || !box) return;
    const line = box.querySelector<HTMLElement>(`[data-at="${readAt.block}-${readAt.line}"]`);
    if (!line) return;
    box.scrollTo({ top: line.offsetTop - box.clientHeight * READING_LINE, behavior: "smooth" });
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
      setNotice(t(reply.data.message_key ?? "creator.teleprompter.followFailed"));
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
  useEffect(() => {
    window.__lunaTeleprompterFollow = () => {
      if (followRef.current === "off") void startFollowRef.current();
    };
    return () => {
      delete window.__lunaTeleprompterFollow;
    };
  }, []);

  const mark = async () => {
    const box = scrollRef.current;
    const at = readAt ?? (box ? lineAtReadingLine(box) : null);
    if (!at) return;
    const reply = await commands.teleprompterMark(at);
    if (reply.status !== "ok") {
      setNotice(vaultErrorText(reply.error));
      return;
    }
    setPendingMarks(reply.data);
  };

  const stepper = (key: TeleprompterNumberKey, label: string, pref: NumberPref) => (
    <div className="luna-teleprompter__stepper" role="group" aria-label={label}>
      <IconButton
        density="sm"
        label={t("creator.teleprompter.less", { what: label })}
        icon={<MinusIcon size={12} />}
        disabled={pref.value <= pref.min}
        onClick={() => prefs.stepNumber(key, -1)}
      />
      <span className="luna-teleprompter__value">{label} {pref.value}</span>
      <IconButton
        density="sm"
        label={t("creator.teleprompter.more", { what: label })}
        icon={<PlusIcon size={12} />}
        disabled={pref.value >= pref.max}
        onClick={() => prefs.stepNumber(key, 1)}
      />
    </div>
  );

  return (
    <main className="luna-teleprompter" data-mirror={prefs.mirror || undefined}>
      <header className="luna-teleprompter__bar" onPointerDown={onPointerDown}>
        <span className="luna-teleprompter__title">{content?.title || t("creator.teleprompter.title")}</span>
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          aria-pressed={follow === "on"}
          disabled={!content?.path || follow === "starting"}
          onClick={() => void (follow === "on" ? stopFollow() : startFollow())}
        >
          <MicIcon size={12} />
          {t(follow === "on" ? "creator.teleprompter.followStop" : "creator.teleprompter.follow")}
        </button>
        <button
          type="button"
          className="luna-btn luna-btn--primary luna-btn--sm"
          disabled={!content?.path || follow !== "off"}
          onClick={() => setPlaying((value) => !value)}
        >
          {playing ? <PauseIcon size={12} /> : <PlayIcon size={12} />}
          {t(playing ? "creator.teleprompter.pause" : "creator.teleprompter.play")}
        </button>
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          title={t("creator.teleprompter.markHint")}
          disabled={!content?.path}
          onClick={() => void mark()}
        >
          {t("creator.teleprompter.mark")}
        </button>
        {stepper("teleprompter.speed", t("creator.teleprompter.speed"), prefs.speed)}
        {stepper("teleprompter.font_size", t("creator.teleprompter.fontSize"), prefs.fontSize)}
        {stepper("teleprompter.line_width", t("creator.teleprompter.lineWidth"), prefs.lineWidth)}
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          aria-pressed={prefs.mirror}
          onClick={() => prefs.setMirror(!prefs.mirror)}
        >
          {t("creator.teleprompter.mirror")}
        </button>
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          onClick={() => {
            scrollRef.current?.scrollTo({ top: 0 });
            if (follow === "on") {
              setReadAt(null);
              void commands.teleprompterFollowReset();
            }
          }}
        >
          {t("creator.teleprompter.top")}
        </button>
        <IconButton
          label={t("creator.teleprompter.close")}
          icon={<CloseIcon size={14} />}
          onClick={() => {
            setPlaying(false);
            // 收起即停跟读：面板看不见了，不该还在听麦克风。
            void stopFollow().then(() => commands.teleprompterHide());
          }}
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
        {notice || pendingMarks > 0 ? (
          <p className="luna-teleprompter__notice" role="status">
            {notice ?? t("creator.teleprompter.markPending", { count: pendingMarks })}
          </p>
        ) : null}
        {error ? (
          <p className="luna-teleprompter__empty" role="status">
            {error}
          </p>
        ) : !content?.path ? (
          <p className="luna-teleprompter__empty">{t("creator.teleprompter.empty")}</p>
        ) : (
          <div className="luna-teleprompter__text">
            {content.blocks.map((block, blockIndex) => (
              <section key={`${blockIndex}-${block.heading ?? ""}`} className="luna-teleprompter__block">
                {block.heading ? <h2 className="luna-teleprompter__heading">{block.heading}</h2> : null}
                {block.lines.map((line, lineIndex) => (
                  <p
                    key={lineIndex}
                    className="luna-teleprompter__line"
                    data-at={`${blockIndex}-${lineIndex}`}
                    data-current={(readAt?.block === blockIndex && readAt.line === lineIndex) || undefined}
                  >
                    {line.marker ? <span className="luna-teleprompter__marker">{line.marker}</span> : null}
                    {line.text}
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
        aria-label={t("creator.teleprompter.resize")}
        title={t("creator.teleprompter.resize")}
        onPointerDown={(event) => {
          event.preventDefault();
          void getCurrentWebviewWindow().startResizeDragging("SouthEast");
        }}
      />
    </main>
  );
}
