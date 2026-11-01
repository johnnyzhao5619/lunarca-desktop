// 文本朗读（TTS）前端接线（TDD-05 §9 / ADR-022）。
//
// 后端 `TtsController` 是**单一发声者**（跨 HUD 弹窗与笔记编辑器多窗口共享），状态经全局事件
// `tts://state` 广播。此处用**模块级单例 store**（一次订阅 + 初始快照，`useSyncExternalStore`
// 分发）避免每个按钮各起一个监听；各表面记住自己 `speak()` 返回的 `session_id`，据此判断
// 「当前发声者是不是我」（不是我即复位），实现跨窗口抢占的一致 UI。
//
// 遵「前端零业务」：规划断句/剥 Markdown/选嗓音/播放全在 Rust，此处只发命令 + 渲染状态。

import { useCallback, useEffect, useState, useSyncExternalStore } from "react";

import { commands, events, type TtsStateChanged } from "../../ipc/bindings";

const IDLE: TtsStateChanged = {
  status: "idle",
  session_id: 0,
  scope: "",
  index: 0,
  total: 0,
  range_from: 0,
  range_to: 0,
};

let current: TtsStateChanged = IDLE;
const listeners = new Set<() => void>();
let started = false;

function notify(): void {
  for (const listener of listeners) listener();
}

function setState(next: TtsStateChanged): void {
  current = next;
  notify();
}

function ensureStarted(): void {
  if (started) return;
  started = true;
  // 初始快照 + 订阅广播（单例，进程内仅一次）。
  void commands
    .ttsState()
    .then(setState)
    .catch(() => {});
  void events.ttsState.listen((event) => setState(event.payload));
}

function subscribe(callback: () => void): () => void {
  ensureStarted();
  listeners.add(callback);
  return () => {
    listeners.delete(callback);
  };
}

function getSnapshot(): TtsStateChanged {
  return current;
}

/** 订阅全局朗读状态快照（单例 store）。 */
export function useTtsState(): TtsStateChanged {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

export type SpeakRequest = {
  /** 要朗读的文本（编辑器传源码，HUD 传原文/译文）。 */
  text: string;
  /** 是否按 Markdown 剥离（编辑器全文=true；选区/HUD=false）。 */
  markdown: boolean;
  /** 显式语言（BCP-47 或 `zh`/`en`；`null` = 后端本地检测）。 */
  lang?: string | null;
  /** 发起表面标识（如 `hud:source` / `editor:<path>`），随状态回传供认领。 */
  scope: string;
};

/** 朗读控制器：状态 + 命令封装。 */
export function useTts(): {
  state: TtsStateChanged;
  speak: (request: SpeakRequest) => Promise<TtsStateChanged>;
  pause: () => Promise<TtsStateChanged>;
  resume: () => Promise<TtsStateChanged>;
  stop: () => Promise<TtsStateChanged>;
} {
  const state = useTtsState();
  const speak = useCallback(
    (request: SpeakRequest) =>
      commands.ttsSpeak(request.text, request.markdown, request.lang ?? null, request.scope),
    [],
  );
  const pause = useCallback(() => commands.ttsPause(), []);
  const resume = useCallback(() => commands.ttsResume(), []);
  const stop = useCallback(() => commands.ttsStop(), []);
  return { state, speak, pause, resume, stop };
}

/** 本平台是否支持朗读（仅 macOS 有原生实现）；不支持时各表面隐藏入口。 */
export function useTtsAvailable(): boolean {
  const [available, setAvailable] = useState(false);
  useEffect(() => {
    let alive = true;
    void commands
      .ttsAvailable()
      .then((value) => {
        if (alive) setAvailable(value);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);
  return available;
}
