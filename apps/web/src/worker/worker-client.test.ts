/** 主线程 Worker 句柄：终止之后不留引用，在途与之后的命令都以 `worker_terminated` 结束。 */

import { describe, expect, it, vi } from "vitest";
import type { WorkerMessage, WorkerRequest } from "./protocol";
import { TERMINATED, WorkerClient, WorkerError, type WorkerLike } from "./worker-client";

function fakeWorker() {
  const posted: WorkerRequest[] = [];
  const worker: WorkerLike & { posted: WorkerRequest[]; reply(message: WorkerMessage): void } = {
    posted,
    onmessage: null,
    postMessage: (message) => posted.push(message),
    terminate: vi.fn(),
    reply(message) {
      this.onmessage?.(new MessageEvent("message", { data: message }));
    },
  };
  return worker;
}

describe("WorkerClient", () => {
  it("按 id 配对应答；失败只带稳定码；事件交给监听者", async () => {
    const worker = fakeWorker();
    const client = new WorkerClient(worker);
    const events: unknown[] = [];
    client.onEvent((event) => events.push(event));
    const ok = client.call("info", {});
    const bad = client.call("list_vaults", {});
    worker.reply({ id: worker.posted[1].id, ok: false, code: "sync_not_entitled" });
    worker.reply({ id: worker.posted[0].id, ok: true, value: { locked: false } });
    worker.reply({ event: "session_locked" });
    await expect(ok).resolves.toEqual({ locked: false });
    await expect(bad).rejects.toMatchObject({ code: "sync_not_entitled" });
    expect(events).toEqual([{ event: "session_locked" }]);
  });

  it("terminate：终止 Worker、摘掉回调、拒绝在途命令，之后的命令也立即拒绝", async () => {
    const worker = fakeWorker();
    const client = new WorkerClient(worker);
    const listener = vi.fn();
    client.onEvent(listener);
    const pending = client.call("read", { file_id: "f" });
    client.terminate();
    expect(worker.terminate).toHaveBeenCalledOnce();
    expect(worker.onmessage).toBeNull();
    expect(client.alive).toBe(false);
    await expect(pending).rejects.toBeInstanceOf(WorkerError);
    await expect(pending).rejects.toMatchObject({ code: TERMINATED });
    await expect(client.call("info", {})).rejects.toMatchObject({ code: TERMINATED });
    expect(listener).not.toHaveBeenCalled();
  });
});
