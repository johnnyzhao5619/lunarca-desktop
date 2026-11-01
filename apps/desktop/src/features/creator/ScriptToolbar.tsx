/**
 * 脚本工具栏（ADR-165 ⑦ / PRD-19-038）：写脚本时的动作都在编辑器上方这一行——提词器、口述一段、
 * AI 改稿（精简 / 重写开头 / 改成口语 / 整理口述），右端是全篇时长，点开是语速（来源、用我的录音
 * 校准、恢复默认）。
 *
 * 这些此前堆在默认折叠的上下文面板「大纲」里；那里只回答「这篇笔记有哪些标题」，动作不属于它。
 * 时长与语速由 Rust 算（`script_outline` / 校准命令），改稿走与选区气泡同一个 AI 面板
 * （`requestScriptRewrite`：没有选区时编辑器先选中光标所在那一段），这里只发命令与事件。
 */

import { useState, type MouseEvent as ReactMouseEvent } from "react";
import { useTranslation } from "react-i18next";

import { commands, type ScriptOutlineDto } from "../../ipc/bindings";
import { useContextMenu } from "../../components/ContextMenuHost";
import { ChevronDownIcon } from "../../components/ui/Icon";
import { notifyScriptRateChanged, requestScriptRewrite, type ScriptRewriteAction } from "../../lib/app-events";
import { buildMenu } from "../../lib/context-menu";
import { formatClock } from "../../lib/format-duration";
import { vaultErrorText } from "../../lib/vault-error";
import { ScriptDictation } from "./ScriptDictation";

/** AI 改稿的四个动作（与选区 AI 菜单「脚本」组同一组、同一顺序）。 */
const REWRITES: readonly { action: ScriptRewriteAction; key: string }[] = [
  { action: "script_tighten", key: "tighten" },
  { action: "script_hook", key: "hook" },
  { action: "script_spoken", key: "spoken" },
  { action: "script_dictation", key: "dictation" },
];

export interface ScriptToolbarProps {
  path: string;
  /** Rust 给的分段与时长；编辑器还没算出来时为 `null`。 */
  outline: ScriptOutlineDto | null;
  /** 操作失败：交给页面就近说（项目工作区页头下面那一条）。 */
  onError: (message: string) => void;
}

export function ScriptToolbar({ path, outline, onError }: ScriptToolbarProps) {
  const { t } = useTranslation("workspace");
  const { t: tEditor } = useTranslation("editor");
  const menu = useContextMenu();
  const [rateOpen, setRateOpen] = useState(false);
  const [calibrating, setCalibrating] = useState(false);
  const [rateNotice, setRateNotice] = useState<string | null>(null);

  const openTeleprompter = async () => {
    const reply = await commands.teleprompterOpen(path, false);
    if (reply.status === "error") onError(vaultErrorText(reply.error));
  };

  const openRewriteMenu = (event: ReactMouseEvent) => {
    menu.open(
      event,
      buildMenu(
        REWRITES.map(({ action, key }) => ({
          kind: "action" as const,
          id: action,
          label: tEditor(`floating.script.${key}`),
          onSelect: () => requestScriptRewrite({ path, action }),
        })),
      ),
    );
  };

  const calibrate = async () => {
    setCalibrating(true);
    setRateNotice(null);
    const result = await commands.creatorCalibrateSpeakingRate();
    setCalibrating(false);
    if (result.status !== "ok") {
      setRateNotice(vaultErrorText(result.error));
      return;
    }
    if (result.data.kind === "not_enough") {
      setRateNotice(t("creator.script.notEnough", { minutes: result.data.minutes ?? 0 }));
      return;
    }
    setRateNotice(t("creator.script.calibrated", { recordings: result.data.recordings }));
    notifyScriptRateChanged();
  };

  const resetRate = async () => {
    setRateNotice(null);
    const result = await commands.creatorResetSpeakingRate();
    if (result.status !== "ok") {
      setRateNotice(vaultErrorText(result.error));
      return;
    }
    notifyScriptRateChanged();
  };

  return (
    <div className="luna-script__toolbar-wrap">
      <div className="luna-script__toolbar" role="toolbar" aria-label={t("creator.script.toolbarLabel")}>
        <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={() => void openTeleprompter()}>
          {t("creator.script.teleprompter")}
        </button>
        <ScriptDictation path={path} />
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          aria-haspopup="menu"
          onClick={openRewriteMenu}
        >
          {t("creator.script.rewrite")}
          <ChevronDownIcon size={12} />
        </button>
        <span className="luna-script__spacer" />
        {outline ? (
          <button
            type="button"
            className="luna-script__duration"
            aria-expanded={rateOpen}
            aria-controls="script-rate"
            onClick={() => setRateOpen((open) => !open)}
          >
            {t(outline.calibrated ? "creator.script.total" : "creator.script.totalApprox", {
              duration: formatClock(outline.total_seconds * 1000),
            })}
            <ChevronDownIcon size={12} />
          </button>
        ) : null}
      </div>
      {outline && rateOpen ? (
        <div className="luna-script__rate" id="script-rate">
          <span>
            {t(outline.calibrated ? "creator.script.rateCalibrated" : "creator.script.rateDefault", {
              cjk: Math.round(outline.cjk_per_minute ?? 0),
              words: Math.round(outline.words_per_minute ?? 0),
            })}
          </span>
          {outline.calibrated ? (
            <span className="luna-script__hint">
              {t("creator.script.sample", { recordings: outline.recordings, minutes: outline.sample_minutes ?? 0 })}
            </span>
          ) : null}
          <span className="luna-script__rate-actions">
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-btn--sm"
              disabled={calibrating}
              onClick={() => void calibrate()}
            >
              {t(outline.calibrated ? "creator.script.recalibrate" : "creator.script.calibrate")}
            </button>
            {outline.calibrated ? (
              <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={() => void resetRate()}>
                {t("creator.script.resetRate")}
              </button>
            ) : null}
          </span>
          {rateNotice ? (
            <span className="luna-script__hint" role="status">
              {rateNotice}
            </span>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
