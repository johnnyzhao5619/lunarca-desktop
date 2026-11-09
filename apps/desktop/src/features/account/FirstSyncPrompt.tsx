/**
 * 登录官方云后的一屏（ADR-178 ② ④）：开了哪个库、与云端怎么合，复用开启向导。挂在工作区根上，
 * 账户页登录、首启引导登录与已登录的老用户走同一条路；要不要问全由 Rust 判定（前端零业务，ADR-003）。
 */

import { useEffect, useState } from "react";
import { createPortal } from "react-dom";

import { useAccount } from "../../hooks/use-account";
import { commands, type FirstLoginOfferDto, type SyncOverviewDto } from "../../ipc/bindings";
import { SyncSetupDialog } from "./SyncSetupDialog";

export function FirstSyncPrompt() {
  const account = useAccount();
  const signedIn = account?.phase.phase === "signed_in";
  const [offer, setOffer] = useState<FirstLoginOfferDto | null>(null);
  const [overview, setOverview] = useState<SyncOverviewDto | null>(null);

  useEffect(() => {
    if (!signedIn) {
      setOffer(null);
      return;
    }
    let live = true;
    void Promise.all([commands.syncFirstLoginOffer(), commands.syncOverview()]).then(([found, sync]) => {
      if (!live || found.status === "error" || sync.status === "error") return;
      setOverview(sync.data);
      setOffer(found.data);
    });
    return () => {
      live = false;
    };
  }, [signedIn]);

  if (!offer || !overview) return null;
  const storage = overview.storage;
  const remainingBytes =
    storage?.limit.kind === "limited" && storage.limit.bytes !== null && storage.used_bytes !== null
      ? Math.max(0, storage.limit.bytes - storage.used_bytes)
      : null;
  return createPortal(
    <SyncSetupDialog
      vaultPath={offer.vault_path}
      vaultName={offer.vault_name}
      takeOver={false}
      localBytes={offer.local_bytes}
      remainingBytes={remainingBytes}
      linkedRemotes={overview.vaults.map((v) => v.remote_vault_id).filter((id): id is string => id !== null)}
      firstLogin={{ recordingBytes: offer.recording_bytes ?? 0 }}
      onClose={() => {
        setOffer(null);
        void commands.syncFirstLoginDecided(offer.vault_path);
      }}
    />,
    document.body,
  );
}
