import { useEffect } from "react";
import { commands, events } from "../../ipc/bindings";
import { flushEditorSaves, holdEditorInputs } from "./editor-save-guard";

/** Rust 退出请求只以本窗口的保存回执应答；正文不穿过退出协议。 */
export function useEditorExitGuard() {
  useEffect(() => {
    const unlisten = events.editorFlushRequested.listen(async ({ payload }) => {
      holdEditorInputs(true);
      const saved = await flushEditorSaves();
      await commands.editorFlushReply(payload.request_id, saved);
    });
    const release = events.editorFlushCancelled.listen(() => holdEditorInputs(false));
    return () => { void unlisten.then((dispose) => dispose()); void release.then((dispose) => dispose()); holdEditorInputs(false); };
  }, []);
}
