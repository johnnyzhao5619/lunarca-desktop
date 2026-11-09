import workerUrl from "./worker.ts?worker&url";
import { trustedScriptUrl } from "../lib/trusted-types";
import { WorkerClient } from "./worker-client";

/** 起一个新的专用 Worker（模块型，Vite 单独打包；入口地址经 Trusted Types policy，TDD-17 §6.1）。 */
export function spawnWorker(): WorkerClient {
  return new WorkerClient(new Worker(trustedScriptUrl(workerUrl), { type: "module", name: "lunarca" }));
}
