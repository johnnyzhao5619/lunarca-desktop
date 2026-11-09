/**
 * 全文索引的状态（PRD-21-041 / 042）：没建、正在下载正文（进度）、可用。索引在 Worker 里，这里只问状态、发起与
 * 取消；建的时候每隔一会儿取一次进度。
 */

import { useCallback, useEffect, useState } from "react";
import type { FulltextState } from "../../worker/protocol";
import { WorkerError, type WorkerClient } from "../../worker/worker-client";

/** 正在建时多久取一次进度。 */
const PROGRESS_POLL_MS = 300;

const NONE: FulltextState = { state: "none", done: 0, total: 0 };

export interface Fulltext {
  state: FulltextState;
  /** 上一次没建成的原因（取消不算）。 */
  problem: { code: string; param?: number } | null;
  start(): void;
  cancel(): void;
}

export function useFulltext(client: WorkerClient): Fulltext {
  const [state, setState] = useState<FulltextState>(NONE);
  const [problem, setProblem] = useState<Fulltext["problem"]>(null);

  useEffect(() => {
    let active = true;
    void client.call("fulltext_state", {}).then(
      (next) => {
        if (active) setState(next);
      },
      () => {},
    );
    return () => {
      active = false;
    };
  }, [client]);

  useEffect(() => {
    if (state.state !== "building") return;
    const timer = window.setInterval(() => {
      void client.call("fulltext_state", {}).then(
        (next) => setState((current) => (current.state === "building" && next.state === "building" ? next : current)),
        () => {},
      );
    }, PROGRESS_POLL_MS);
    return () => window.clearInterval(timer);
  }, [client, state.state]);

  const start = useCallback(() => {
    setProblem(null);
    setState({ state: "building", done: 0, total: 0 });
    void client.call("build_fulltext", {}).then(setState, (error: unknown) => {
      setState(NONE);
      const code = error instanceof WorkerError ? { code: error.code, param: error.param } : { code: "internal" };
      if (code.code !== "fulltext_cancelled") setProblem(code);
    });
  }, [client]);

  const cancel = useCallback(() => {
    void client.call("cancel_fulltext", {}).catch(() => {});
  }, [client]);

  return { state, problem, start, cancel };
}
