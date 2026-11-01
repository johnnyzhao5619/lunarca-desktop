/**
 * 处理送达的 `lunarca://` 库内容链接（ADR-141，笔记库对标轮 W4a）。
 *
 * 认库、路径边界、文件在不在、标题行号全由 Rust 判完（`vault_link_pending`）；这里只按结果做：
 * - 被拒 → 说清为什么（库不在这台设备上、这篇不在了……），而不是什么都不发生；
 * - 指向别的库 → **先问**再切（ADR-141 ①）。切库会让工作区整个重挂载，请求留在 Rust，
 *   重挂载后这里再取一次、执行；
 * - `open` / `search` 直接执行（读直通）；
 * - `new` 只弹预填名字的输入卡，用户确认后才写盘（写须确认）。
 *
 * 挂载时取一次（冷启动由链接拉起时，链接比界面先到），之后随 `vault-link://received` 再取。
 */

import { useEffect, useRef } from "react";
import { commands, events, type VaultLinkRejectionDto } from "../../ipc/bindings";

export interface VaultLinkHandlers {
  /** 问用户要不要切到那个库（`true` = 切）。 */
  confirmSwitch: (vaultName: string) => Promise<boolean>;
  /** 切库（走既有的切库流程：预检、代价确认、过渡态）。 */
  switchVault: (path: string, name: string) => void;
  open: (path: string, line: number | null, headingIndex: number | null) => void;
  search: (query: string) => void;
  /** 弹预填的新建卡；确认后建好并打开。 */
  createNamed: (name: string) => Promise<void>;
  report: (reason: VaultLinkRejectionDto) => void;
}

export function useVaultLinks(handlers: VaultLinkHandlers): void {
  const handlersRef = useRef(handlers);
  handlersRef.current = handlers;
  const busy = useRef(false);

  useEffect(() => {
    let disposed = false;
    const process = async () => {
      if (busy.current) return;
      busy.current = true;
      try {
        const pending = await commands.vaultLinkPending();
        if (disposed || pending === null) return;
        const h = handlersRef.current;
        if (pending.kind === "rejected") {
          h.report(pending.reason);
          return;
        }
        if (pending.needs_switch) {
          if (await h.confirmSwitch(pending.vault_name)) {
            // 请求留在 Rust：切完工作区重挂载，挂载时再取一次就是在目标库里执行。
            h.switchVault(pending.vault_path, pending.vault_name);
          } else {
            await commands.vaultLinkDone();
          }
          return;
        }
        // 先清再执行：执行里可能弹卡等用户，期间再来一条链接应当是新的那条。
        await commands.vaultLinkDone();
        const action = pending.action;
        if (action.kind === "open") {
          h.open(action.path, action.line, action.heading_index);
        } else if (action.kind === "search") {
          h.search(action.query);
        } else {
          await h.createNamed(action.name);
        }
      } finally {
        busy.current = false;
      }
    };
    void process();
    const unlisten = events.vaultLinkReceived.listen(() => void process());
    return () => {
      disposed = true;
      void unlisten.then((dispose) => dispose());
    };
  }, []);
}
