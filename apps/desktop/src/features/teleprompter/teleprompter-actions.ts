/**
 * 提词器的送稿与打开动作（ADR-176 ⑨）：首页、浮窗与命令面板共用。建稿、转换、打开都在 Rust，
 * 这里只发命令、把错误转成可读的话。
 */

import { commands, type TeleprompterDraftSourceDto } from "../../ipc/bindings";
import { goTo } from "../../lib/navigate";
import { vaultErrorText } from "../../lib/vault-error";

/** 用提词器打开一篇稿子；失败时给出可读原因。 */
export async function openInTeleprompter(path: string): Promise<string | null> {
  const reply = await commands.teleprompterOpen(path, false);
  return reply.status === "ok" ? null : vaultErrorText(reply.error);
}

/**
 * 新建一篇提词稿（Rust 建好即用提词器打开，ADR-176 ⑨）；返回失败原因（成功为 `null`）。空稿建好后
 * 还在编辑器里打开它——空稿是拿来写的。
 */
export async function createTeleprompterDraft(source: TeleprompterDraftSourceDto): Promise<string | null> {
  const created = await commands.teleprompterNewDraft(source);
  if (created.status !== "ok") return vaultErrorText(created.error);
  if (source.kind === "empty") goTo(`notes://${created.data}`);
  return null;
}

/** 拖进来的文件转成提词稿并打开第一篇；返回失败原因（成功为 `null`）。 */
export async function importTeleprompterFiles(paths: readonly string[]): Promise<string | null> {
  const reply = await commands.teleprompterImportFiles([...paths]);
  return reply.status === "ok" ? null : vaultErrorText(reply.error);
}
