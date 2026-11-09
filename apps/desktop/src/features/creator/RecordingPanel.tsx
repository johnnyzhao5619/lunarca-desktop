/**
 * 录制页签（ADR-165 ⑧ / PRD-19-039）：开录前在这里看一眼，再交给提词器。
 *
 * - **开录前检查**：跟读要麦克风与实时转写。判定与修复和「添加内容」的语音速记是同一套
 *   （`creator_recording_check` 复用 `create_actions` 的判定，修复走 `runCreateActionRepair`），
 *   缺什么就在这一行给去处。
 * - **主按钮「开始跟读」**：打开提词器并直接开跟读（`teleprompter_open(path, true)`）；不跟读时
 *   也可只打开提词器手动滚动。
 * - **重来标记**：脚本里已写回的 `> [!retake]`（`creator_script_retakes`），点一条回到脚本那一行。
 *
 * 判定全在 Rust，这里只发命令、渲染结果（ADR-003）。
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  commands,
  events,
  type CreateActionStateDto,
  type RecordingCheckDto,
  type RetakeNoteDto,
} from "../../ipc/bindings";
import { CheckIcon } from "@lunarca/editor-core/Icon";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { vaultErrorText } from "../../lib/vault-error";
import { useTeleprompterPrefs } from "../teleprompter/use-teleprompter-prefs";
import {
  CREATE_ACTION_REASON_KEYS,
  CREATE_ACTION_REPAIR_LABEL_KEYS,
  runCreateActionRepair,
  type CreateActionContext,
} from "../capture/create-actions";

export interface RecordingPanelProps {
  /** 这个项目的脚本。 */
  script: string;
  /** 修复动作要用的入口（与「添加内容」同一份）。 */
  createActions: CreateActionContext;
  /** 点一条重来标记：回到脚本页签的那一行。 */
  onOpenScriptAt: (line: number) => void;
  /** 操作失败：交给页面就近说（项目工作区页头下面那一条）。 */
  onError: (message: string) => void;
}

export function RecordingPanel({ script, createActions, onOpenScriptAt, onError }: RecordingPanelProps) {
  const { t } = useTranslation("workspace");
  const [check, setCheck] = useState<RecordingCheckDto | null>(null);
  const [retakes, setRetakes] = useState<RetakeNoteDto[] | null>(null);
  const checkRequestRef = useRef(0);
  const retakeRequestRef = useRef(0);

  const loadCheck = () => {
    const request = ++checkRequestRef.current;
    void commands.creatorRecordingCheck().then((result) => {
      if (request === checkRequestRef.current) setCheck(result);
    });
  };
  const loadRetakes = () => {
    const request = ++retakeRequestRef.current;
    void commands.creatorScriptRetakes(script).then((reply) => {
      if (request !== retakeRequestRef.current) return;
      setRetakes(reply.status === "ok" ? reply.data : []);
    });
  };

  useEffect(loadCheck, []);
  // 授权、模型、录音会话一变，「添加内容」那份快照就会重发：借它重查；从系统设置回来也重查。
  useEffect(() => {
    const unlisten = events.createActionsStatesChanged.listen(() => loadCheck());
    window.addEventListener("focus", loadCheck);
    return () => {
      void unlisten.then((dispose) => dispose());
      window.removeEventListener("focus", loadCheck);
    };
  }, []);

  useEffect(loadRetakes, [script]);
  useVaultReload(loadRetakes);
  // 停止跟读时标记写回脚本（一次普通写盘）：跟着重取。
  useEffect(() => {
    const unlisten = events.noteWritten.listen((event) => {
      if (event.payload.path === script) loadRetakes();
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [script]);

  const open = async (follow: boolean) => {
    const reply = await commands.teleprompterOpen(script, follow);
    if (reply.status === "error") onError(vaultErrorText(reply.error));
  };
  // 跟读由用户开、默认关（ADR-176 ④）：关着时这一下就是「开启跟读并开始」——先写开关再开。
  const teleprompter = useTeleprompterPrefs();
  const startFollow = async () => {
    if (!teleprompter.followEnabled && !(await teleprompter.setFollowEnabled(true))) return;
    await open(true);
  };

  const ready = check?.microphone.kind === "available" && check.transcriber.kind === "available";

  const row = (label: string, state: CreateActionStateDto | undefined) => (
    <li className="luna-recording__check" data-ready={state?.kind === "available" || undefined}>
      <span className="luna-recording__check-name">{label}</span>
      {state === undefined ? (
        <span className="luna-creator-page__hint">{t("common:status.loading")}</span>
      ) : state.kind === "available" ? (
        <span className="luna-recording__check-ok">
          <CheckIcon size={12} />
          {t("creator.recording.ready")}
        </span>
      ) : (
        <span className="luna-recording__check-gap">
          <span>{t(CREATE_ACTION_REASON_KEYS[state.reason])}</span>
          {state.kind === "needs_attention" ? (
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-btn--sm"
              onClick={() => {
                runCreateActionRepair(state.repair, createActions);
                loadCheck();
              }}
            >
              {t(CREATE_ACTION_REPAIR_LABEL_KEYS[state.repair.kind])}
            </button>
          ) : null}
        </span>
      )}
    </li>
  );

  return (
    <div className="luna-recording">
      <section className="luna-creator-page__section" aria-labelledby="recording-check">
        <h2 id="recording-check">{t("creator.recording.check")}</h2>
        <ul className="luna-recording__checks">
          {row(t("creator.recording.microphone"), check?.microphone)}
          {row(t("creator.recording.transcriber"), check?.transcriber)}
        </ul>
        <div className="luna-creator-page__row">
          <button type="button" className="luna-btn luna-btn--primary" disabled={!ready} onClick={() => void startFollow()}>
            {t(teleprompter.followEnabled ? "creator.recording.startFollow" : "creator.recording.enableFollow")}
          </button>
          <button type="button" className="luna-btn luna-btn--secondary" onClick={() => void open(false)}>
            {t("creator.recording.openOnly")}
          </button>
        </div>
        <p className="luna-creator-page__hint">{t("creator.recording.hint")}</p>
      </section>

      <section className="luna-creator-page__section" aria-labelledby="recording-retakes">
        <h2 id="recording-retakes">{t("creator.recording.retakes")}</h2>
        {retakes === null ? null : retakes.length === 0 ? (
          <p className="luna-creator-page__hint">{t("creator.recording.noRetakes")}</p>
        ) : (
          <ul className="luna-recording__retakes">
            {retakes.map((retake) => (
              <li key={`${retake.line}-${retake.note}`}>
                <button
                  type="button"
                  className="luna-recording__retake"
                  title={t("creator.recording.retakeHint")}
                  onClick={() => onOpenScriptAt(retake.line)}
                >
                  <span className="luna-recording__retake-text">{retake.text}</span>
                  <span className="luna-recording__retake-note">{retake.note}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
