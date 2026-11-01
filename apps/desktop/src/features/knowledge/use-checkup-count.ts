/**
 * 库体检的发现数（PRD-16-030）。判定与数字全部来自 Rust `checkup_report`。
 *
 * 取数与呈现分开（同 `use-knowledge-pulse`）：`loaded` 让宿主能等三条来源都到齐再一起
 * 画「下一步」那一区——否则它们各自到达、各自决定要不要渲染，弹窗打开后高度会连跳两三次，
 * 而用户正在读的就是那几行。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { commands } from "../../ipc/bindings";
import { useVaultReload } from "../../hooks/use-vault-reload";

export interface CheckupCount {
  /** 值得收拾的地方有几处；`0` = 没有发现（整行不渲染）。 */
  count: number;
  loaded: boolean;
}

export function useCheckupCount(): CheckupCount {
  const [state, setState] = useState<CheckupCount>({ count: 0, loaded: false });
  const disposed = useRef(false);

  const load = useCallback(() => {
    void commands.checkupReport().then((result) => {
      if (disposed.current) {
        return;
      }
      if (result.status !== "ok") {
        // 取不到就是没有发现：体检是增值信息，出不来不该占一行骨架屏，更不该卡住整区。
        setState({ count: 0, loaded: true });
        return;
      }
      // 积压不计入：脉搏那一行已经在说「有 N 篇待整理」，体检再数一遍就是同一屏里把
      // 同一件事说两遍。这一行要报的是库本身该收拾的地方（判定由 Rust 下发）。
      const hygiene = result.data.findings.filter((finding) => !finding.backlog);
      setState({ count: hygiene.length + result.data.truncated, loaded: true });
    });
  }, []);

  useEffect(() => {
    disposed.current = false;
    load();
    return () => {
      disposed.current = true;
    };
  }, [load]);

  // 库变了就重数（V1b）：启动时这一行先于开库那一轮扫描挂上，数的是半截索引——扫描做完
  // 由 `useVaultReload` 再数一次，不让半截的数字一直挂着。
  useVaultReload(load);

  return state;
}
