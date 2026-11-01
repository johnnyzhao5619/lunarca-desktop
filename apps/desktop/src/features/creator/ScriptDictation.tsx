/**
 * 脚本工具栏里的「口述一段」（ADR-162 ⑮ / ADR-165 ⑦，PRD-19-013）：点一下开始听，再点一下结束；识别出的原话
 * 插在脚本光标处并选中，「整理口述」就在选区气泡的 AI ▸ 脚本 里。
 *
 * 采集、识别与「原话不过自动优化」全在 Rust（`script_voice_input_start`，sink `ScriptDraft`）；
 * 这里只发命令、显示实时识别的字，把原话交给活跃编辑器插入（`requestScriptInsert`，ADR-003）。
 */

import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Channel } from "@tauri-apps/api/core";
import { commands, type VoiceInputEvent } from "../../ipc/bindings";
import { requestScriptInsert } from "../../lib/app-events";
import { keyToMessage } from "../voice-input/voice-message";
import { vaultErrorText } from "../../lib/vault-error";
import { failureMessage, hasFailureMessage } from "../../i18n/failure";

type Phase = "idle" | "listening" | "recognizing";

export function ScriptDictation({ path }: { path: string }) {
  const { t } = useTranslation("workspace");
  const { t: tVoice } = useTranslation("voice-input");
  const [phase, setPhase] = useState<Phase>("idle");
  const [heard, setHeard] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  /** 这次口述要插进哪一篇（开始那一刻的脚本；中途换了笔记也插回原处）。 */
  const targetRef = useRef(path);

  const onEvent = (event: VoiceInputEvent) => {
    switch (event.kind) {
      case "partial":
        setHeard(`${event.data.text}${event.data.hypothesis}`);
        break;
      case "state":
        if (event.data.status === "recording") setPhase("listening");
        else if (event.data.status === "recognizing" || event.data.status === "landing") setPhase("recognizing");
        else if (event.data.status === "error") {
          setPhase("idle");
          setNotice(keyToMessage(event.data.message_key, event.data.status, tVoice));
        }
        break;
      case "field_input":
        setPhase("idle");
        setHeard("");
        if (event.data.text.trim()) {
          requestScriptInsert({ path: targetRef.current, text: event.data.text, select: true });
          setNotice(t("creator.dictation.inserted"));
        } else {
          setNotice(t("creator.dictation.nothing"));
        }
        break;
      case "failed": {
        const code = event.data.code.toUpperCase();
        setPhase("idle");
        setNotice(hasFailureMessage(code) ? failureMessage(code) : t("creator.dictation.failed"));
        break;
      }
      default:
        break;
    }
  };

  const toggle = async () => {
    if (phase === "listening") {
      setPhase("recognizing");
      const stopped = await commands.voiceInputStop(null);
      if (stopped.status !== "ok") {
        setPhase("idle");
        setNotice(vaultErrorText(stopped.error));
      }
      return;
    }
    setNotice(null);
    setHeard("");
    targetRef.current = path;
    const channel = new Channel<VoiceInputEvent>();
    channel.onmessage = onEvent;
    const reply = await commands.scriptVoiceInputStart(channel);
    if (reply.status !== "ok") {
      setNotice(vaultErrorText(reply.error));
      return;
    }
    if (reply.data.status === "recording") {
      setPhase("listening");
    } else {
      // 麦克风被录音 / 字幕 / 别的语音会话占着：命令照常返回，但没开起来。
      setNotice(keyToMessage(reply.data.message_key, reply.data.status, tVoice));
    }
  };

  return (
    <span className="luna-script__dictation">
      <button
        type="button"
        className="luna-btn luna-btn--secondary luna-btn--sm"
        aria-pressed={phase === "listening"}
        disabled={phase === "recognizing"}
        onClick={() => void toggle()}
      >
        {t(
          phase === "listening"
            ? "creator.dictation.stop"
            : phase === "recognizing"
              ? "creator.dictation.recognizing"
              : "creator.dictation.start",
        )}
      </button>
      {phase !== "idle" && heard ? <span className="luna-script__hint">{heard}</span> : null}
      {notice ? (
        <span className="luna-script__hint" role="status">
          {notice}
        </span>
      ) : null}
    </span>
  );
}
