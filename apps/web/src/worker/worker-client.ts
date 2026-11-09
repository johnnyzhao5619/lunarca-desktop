/**
 * 主线程这一侧的 Worker 句柄（TDD-17 §1）：发命令、等应答、收事件、终止。主线程从不持有令牌或密钥；
 * 锁定与登出之后整个 Worker 被终止（连同其 WASM 内存），句柄随之作废——要继续就新起一个。
 */

import type { CommandMap, CommandName, WorkerEvent, WorkerMessage, WorkerRequest } from "./protocol";
import { isWorkerEvent } from "./protocol";

/** 命令失败：只带稳定码与可选的数值参数（`too_large` 的上限）。 */
export class WorkerError extends Error {
  readonly code: string;
  readonly param: number | undefined;

  constructor(code: string, param?: number) {
    super(code);
    this.code = code;
    this.param = param;
  }
}

/** Worker 已终止之后还有人发命令 / 在途的命令被终止打断。 */
export const TERMINATED = "worker_terminated";

export interface WorkerLike {
  postMessage(message: WorkerRequest): void;
  terminate(): void;
  onmessage: ((message: MessageEvent<WorkerMessage>) => void) | null;
}

export class WorkerClient {
  private worker: WorkerLike | null;
  private sequence = 0;
  private readonly pending = new Map<number, { resolve(value: unknown): void; reject(error: Error): void }>();
  private readonly listeners = new Set<(event: WorkerEvent) => void>();

  constructor(worker: WorkerLike) {
    this.worker = worker;
    worker.onmessage = (message) => this.receive(message.data);
  }

  get alive(): boolean {
    return this.worker !== null;
  }

  call<C extends CommandName>(cmd: C, args: CommandMap[C]["args"]): Promise<CommandMap[C]["result"]> {
    const worker = this.worker;
    if (!worker) return Promise.reject(new WorkerError(TERMINATED));
    const id = ++this.sequence;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (value: unknown) => void, reject });
      worker.postMessage({ id, cmd, args });
    });
  }

  onEvent(listener: (event: WorkerEvent) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** 终止 Worker：在途命令一律以 `worker_terminated` 结束，监听者全部移除，不再留任何引用。 */
  terminate(): void {
    const worker = this.worker;
    this.worker = null;
    if (worker) {
      worker.onmessage = null;
      worker.terminate();
    }
    for (const { reject } of this.pending.values()) reject(new WorkerError(TERMINATED));
    this.pending.clear();
    this.listeners.clear();
  }

  private receive(message: WorkerMessage): void {
    if (isWorkerEvent(message)) {
      for (const listener of this.listeners) listener(message);
      return;
    }
    const waiting = this.pending.get(message.id);
    if (!waiting) return;
    this.pending.delete(message.id);
    if (message.ok) waiting.resolve(message.value);
    else waiting.reject(new WorkerError(message.code, message.param));
  }
}
