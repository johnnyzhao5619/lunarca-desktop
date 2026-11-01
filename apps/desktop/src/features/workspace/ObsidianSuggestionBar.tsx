/**
 * 「沿用 Obsidian 的设置？」提示条（PRD-06-064，笔记库对标轮 V4c）。
 *
 * 打开一个也在用 Obsidian 的库时，Rust 读它的 `.obsidian/app.json`（只读、从不写），把与当前
 * 不同的附件位置和排除列表说出来，给「沿用 / 不用」。只问一次：两种选择都记在库里。
 * 判定、比较与落盘全在 Rust（沿用时 Rust 按此刻重算，不信这里显示的内容），前端零业务。
 * 位置与库外改名提示条相同（编辑区底部），同一套样式。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, events, type ObsidianSuggestionDto } from "../../ipc/bindings";
import { vaultErrorText } from "../../lib/vault-error";

/** 当前的建议：挂载与切库时取一次。 */
export function useObsidianSuggestion(): [ObsidianSuggestionDto | null, () => void] {
  const [suggestion, setSuggestion] = useState<ObsidianSuggestionDto | null>(null);
  useEffect(() => {
    let disposed = false;
    const load = () => {
      void commands.obsidianSuggestion().then((result) => {
        if (!disposed) setSuggestion(result.status === "ok" ? result.data : null);
      });
    };
    load();
    const unlisten = events.vaultSwitched.listen(load);
    return () => {
      disposed = true;
      void unlisten.then((dispose) => dispose());
    };
  }, []);
  return [suggestion, () => setSuggestion(null)];
}

function attachmentLine(
  suggestion: ObsidianSuggestionDto,
  t: (key: string, options?: Record<string, unknown>) => string,
): string | null {
  const folder = suggestion.attachment_folder ?? "";
  switch (suggestion.attachment_location) {
    case "vault_root":
      return t("obsidianSuggestion.attachmentRoot");
    case "note_folder":
      return t("obsidianSuggestion.attachmentNoteFolder");
    case "note_subfolder":
      return t("obsidianSuggestion.attachmentNoteSubfolder", { folder });
    case "specified_folder":
      return t("obsidianSuggestion.attachmentFolder", { folder });
    default:
      return null;
  }
}

export function ObsidianSuggestionBar({
  suggestion,
  onSettled,
  onStatus,
}: {
  suggestion: ObsidianSuggestionDto;
  /** 选过了（沿用或不用）：外层收起提示条。 */
  onSettled: () => void;
  onStatus: (message: string) => void;
}) {
  const { t, i18n } = useTranslation("workspace");
  const [busy, setBusy] = useState(false);
  const attachment = attachmentLine(suggestion, t);
  const list = new Intl.ListFormat(i18n.language, { type: "conjunction" }).format(
    suggestion.exclusions,
  );

  async function settle(accept: boolean) {
    setBusy(true);
    const result = accept
      ? await commands.acceptObsidianSuggestion()
      : await commands.dismissObsidianSuggestion();
    setBusy(false);
    if (result.status === "error") {
      onStatus(vaultErrorText(result.error));
      return;
    }
    if (accept) {
      onStatus(t("obsidianSuggestion.applied"));
    }
    onSettled();
  }

  return (
    <div className="luna-relocation-receipt" role="status">
      {/* 几句话收在同一块里：提示条是「一段说明 + 右端按钮」两栏，散开的句子会被拉成等距几列。 */}
      <span className="luna-obsidian-suggestion__text">
        <span>{t("obsidianSuggestion.title")}</span>
        {attachment !== null && <span>{attachment}</span>}
        {suggestion.exclusions.length > 0 && (
          <span title={list}>
            {t("obsidianSuggestion.exclusions", { count: suggestion.exclusions.length, list })}
          </span>
        )}
        {suggestion.skipped_patterns > 0 && (
          <span>{t("obsidianSuggestion.skipped", { count: suggestion.skipped_patterns })}</span>
        )}
      </span>
      <span className="luna-relocation-receipt__actions">
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          disabled={busy}
          onClick={() => void settle(true)}
        >
          {t("obsidianSuggestion.accept")}
        </button>
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          disabled={busy}
          onClick={() => void settle(false)}
        >
          {t("obsidianSuggestion.dismiss")}
        </button>
      </span>
    </div>
  );
}
