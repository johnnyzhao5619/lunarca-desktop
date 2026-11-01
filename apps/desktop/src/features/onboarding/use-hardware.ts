// 设备算力摘要读取 hook（PRD-14-010）。检测/档位/推荐全在 Rust hardware_summary，
// 此处仅取数供「硬件检测」「模型准备」两步共享（向导壳取一次，避免重复采样）。

import { useEffect, useState } from "react";
import { commands, type HardwareSummaryDto } from "../../ipc/bindings";
import { vaultErrorText } from "../../lib/vault-error";

export interface HardwareProbe {
  summary: HardwareSummaryDto | null;
  error: string | null;
  loading: boolean;
}

export function useHardwareSummary(): HardwareProbe {
  const [summary, setSummary] = useState<HardwareSummaryDto | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void commands.hardwareSummary().then((result) => {
      if (result.status === "ok") {
        setSummary(result.data);
      } else {
        setError(vaultErrorText(result.error));
      }
    });
  }, []);

  return { summary, error, loading: summary === null && error === null };
}
