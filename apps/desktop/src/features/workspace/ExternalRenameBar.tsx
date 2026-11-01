/**
 * 库外改名的提示条（PRD-06-030 ⑤，笔记库对标轮 V3a）。
 *
 * 在访达、`git mv` 或应用没开时把笔记改了名，Rust 按内容认出来、算出还有几处链接指向旧名，
 * 这里只把事实说出来并给「修复 / 忽略」——**默认询问、不自动改**（用户裁决 Q4）。批量外部
 * 变动（`git checkout`、同步盘首次下载）合成一句，不逐条弹。
 *
 * 判定与改写全在 Rust；修复会改写别的笔记，故与应用内改名一样先过保存屏障。提示放在编辑区
 * 底部（与改名回执同一位置），而不是通知：通知只能带跳转目标，带不了「修复」这个动作。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, events, type ExternalRenameDto } from "../../ipc/bindings";
import { vaultErrorText } from "../../lib/vault-error";
import { afterEditorsSaved } from "../editor/editor-save-guard";

type Prompt = NonNullable<Awaited<ReturnType<typeof commands.externalRenamePrompt>>>;

/** 当前的库外改名提示：挂载时取一次（启动扫描可能比这里更早结束），之后随事件重取。 */
export function useExternalRenamePrompt(): Prompt | null {
  const [prompt, setPrompt] = useState<Prompt | null>(null);
  useEffect(() => {
    let disposed = false;
    const load = () => {
      void commands.externalRenamePrompt().then((next) => {
        if (!disposed) setPrompt(next);
      });
    };
    load();
    const unlisten = events.vaultExternalRenames.listen(load);
    return () => {
      disposed = true;
      void unlisten.then((dispose) => dispose());
    };
  }, []);
  return prompt;
}

function noteName(path: string): string {
  const name = path.split("/").pop() ?? path;
  return name.replace(/\.md$/i, "");
}

export function ExternalRenameBar({
  prompt,
  onRepaired,
  onStatus,
}: {
  prompt: Prompt;
  /** 修好了：外层把开着的旧路径标签映射到新路径。 */
  onRepaired: (renames: readonly ExternalRenameDto[]) => void;
  onStatus: (message: string) => void;
}) {
  const { t } = useTranslation("workspace");
  const { t: tCommon } = useTranslation("common");
  const [busy, setBusy] = useState(false);
  const renames = prompt.renames;
  const pairs = renames.map(({ from, to }) => ({ from, to }));
  const links = renames.reduce((sum, rename) => sum + rename.links, 0);
  const only = renames.length === 1 ? renames[0] : null;

  const summary = prompt.batch
    ? t("externalRename.batch", { changes: prompt.changes, renamed: renames.length, links })
    : only
      ? t("externalRename.one", { from: noteName(only.from), to: noteName(only.to), links })
      : t("externalRename.many", { renamed: renames.length, links });

  async function repair() {
    setBusy(true);
    const result = await afterEditorsSaved(
      () => commands.repairExternalRenames(pairs),
      () => ({ status: "error" as const, error: tCommon("saveBlocksMove") }),
    );
    setBusy(false);
    if (result.status === "error") {
      onStatus(vaultErrorText(result.error));
      return;
    }
    onRepaired(renames);
    onStatus(t("relocation.updated", { count: result.data.links, notes: result.data.notes }));
  }

  return (
    <div
      className="luna-relocation-receipt"
      role="status"
      title={only ? `${only.from} → ${only.to}` : undefined}
    >
      <span>{summary}</span>
      <span className="luna-relocation-receipt__actions">
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          disabled={busy}
          onClick={() => void repair()}
        >
          {only ? t("externalRename.repair") : t("externalRename.repairAll")}
        </button>
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          disabled={busy}
          onClick={() => void commands.dismissExternalRenames(pairs)}
        >
          {t("externalRename.dismiss")}
        </button>
      </span>
    </div>
  );
}
