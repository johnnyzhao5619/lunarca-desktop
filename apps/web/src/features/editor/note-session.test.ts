// @vitest-environment happy-dom
/**
 * 一篇笔记的保存编排（TDD-17 §5.1–§5.3、PRD-21-021 / 030 / 031 / 032）：停手 1 秒与最长 10 秒的节拍、四种状态、
 * 在途串行、合并结果以最小差量落回编辑器（在途期间接着打的字不丢）与撤回、冲突副本、断网重发、别处改了时
 * 原地更新或出横幅、只读不发保存。Worker 换成假的。
 */

import { afterEach, describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { CommandMap, CommandName } from "../../worker/protocol";
import { WorkerError } from "../../worker/worker-client";
import { NoteSession, SAVE_IDLE_MS, SAVE_MAX_MS } from "./note-session";

type Reply = unknown | Error;

function harness(options: { text?: string; gate?: string | null } = {}) {
  const calls: Array<{ cmd: CommandName; args: unknown }> = [];
  const replies: Partial<Record<CommandName, Reply[]>> = {};
  let now = 0;
  const timers: Array<{ at: number; run: () => void; id: number; cleared: boolean }> = [];
  let nextId = 1;
  let gate = options.gate ?? null;
  const waiters: Array<() => void> = [];
  const call = (<C extends CommandName>(cmd: C, args: CommandMap[C]["args"]) => {
    calls.push({ cmd, args });
    const queue = replies[cmd] ?? [];
    const reply = queue.shift();
    return new Promise<CommandMap[C]["result"]>((resolve, reject) => {
      const settle = () => (reply instanceof Error ? reject(reply) : resolve(reply as CommandMap[C]["result"]));
      waiters.push(settle);
    });
  }) as <C extends CommandName>(cmd: C, args: CommandMap[C]["args"]) => Promise<CommandMap[C]["result"]>;

  const text = options.text ?? "第一段\n\n第二段";
  const session = new NoteSession(
    {
      call,
      conflictLabel: () => "冲突 网页",
      readOnly: () => gate,
      setTimeout: (run, ms) => {
        const timer = { at: now + ms, run, id: nextId++, cleared: false };
        timers.push(timer);
        return timer.id;
      },
      clearTimeout: (id) => {
        const timer = timers.find((candidate) => candidate.id === id);
        if (timer) timer.cleared = true;
      },
    },
    { file_id: "f", path: "甲.md", rev: 3, text },
    EditorState.create({ doc: text }),
  );
  const parent = document.createElement("div");
  document.body.appendChild(parent);
  const view = new EditorView({ parent, state: EditorState.create({ doc: "" }) });
  // 编辑器的更新监听在 NoteEditor 里；这里直接转给会话。
  const type = (insert: string, at = view.state.doc.length) => {
    const tr = view.state.update({ changes: { from: at, insert } });
    view.dispatch(tr);
    session.docChanged(tr.changes, true);
  };
  session.attach(view);
  const advance = async (ms: number) => {
    now += ms;
    for (const timer of timers.filter((candidate) => !candidate.cleared && candidate.at <= now)) {
      timer.cleared = true;
      timer.run();
    }
    await Promise.resolve();
  };
  /** 让在途的命令一个个落定（按发出的顺序）。 */
  const settleAll = async () => {
    for (let round = 0; round < 10; round += 1) {
      while (waiters.length > 0) waiters.shift()?.();
      await new Promise((resolve) => setTimeout(resolve, 0));
    }
  };
  return {
    session,
    view,
    calls,
    replies,
    type,
    advance,
    settleAll,
    setGate: (next: string | null) => {
      gate = next;
    },
    saves: () => calls.filter((c) => c.cmd === "save").map((c) => c.args as CommandMap["save"]["args"]),
  };
}

afterEach(() => {
  document.body.replaceChildren();
});

describe("自动保存的节拍与状态", () => {
  it("停手 1 秒保存；存上之前是「保存中」，存上之后「已保存」，基线换成新 rev", async () => {
    const h = harness();
    h.replies.save = [{ kind: "saved", file_id: "f", path: "甲.md", rev: 4 }];
    h.type("，补一句");
    expect(h.session.getSnapshot().status.kind).toBe("saving");
    await h.advance(SAVE_IDLE_MS - 1);
    expect(h.saves()).toHaveLength(0);
    await h.advance(1);
    expect(h.saves()).toEqual([{ file_id: "f", base_rev: 3, text: "第一段\n\n第二段，补一句", conflict_label: "冲突 网页" }]);
    await h.settleAll();
    expect(h.session.getSnapshot().status.kind).toBe("saved");
    expect(h.session.baseRev).toBe(4);
    expect(h.session.unsaved).toBe(false);
  });

  it("连续输入不停手：自第一下起最长 10 秒强制保存一次", async () => {
    const h = harness();
    h.replies.save = [{ kind: "saved", file_id: "f", path: "甲.md", rev: 4 }];
    for (let elapsed = 0; elapsed < SAVE_MAX_MS; elapsed += 500) {
      h.type("字");
      await h.advance(500);
    }
    expect(h.saves()).toHaveLength(1);
  });

  it("同一篇在途时不并发：在途期间又要存，落定之后以新的 rev 为基线再存一次", async () => {
    const h = harness();
    h.replies.save = [
      { kind: "saved", file_id: "f", path: "甲.md", rev: 4 },
      { kind: "saved", file_id: "f", path: "甲.md", rev: 5 },
    ];
    h.type("一");
    void h.session.flush();
    h.type("二");
    void h.session.flush();
    expect(h.saves()).toHaveLength(1);
    await h.settleAll();
    expect(h.saves().map((s) => s.base_rev)).toEqual([3, 4]);
    expect(h.saves()[1].text.endsWith("一二")).toBe(true);
    expect(h.session.getSnapshot().status.kind).toBe("saved");
  });

  it("断网：状态写「离线」，改动留在本页；联网后立刻用同一份内容重发", async () => {
    const h = harness();
    h.replies.save = [new WorkerError("network_unavailable"), { kind: "saved", file_id: "f", path: "甲.md", rev: 4 }];
    h.type("离线时写的");
    await h.advance(SAVE_IDLE_MS);
    await h.settleAll();
    expect(h.session.getSnapshot().status.kind).toBe("offline");
    expect(h.session.unsaved).toBe(true);
    h.session.retryNow();
    await h.settleAll();
    expect(h.saves()).toHaveLength(2);
    expect(h.saves()[1]).toEqual(h.saves()[0]);
    expect(h.session.getSnapshot().status.kind).toBe("saved");
  });

  it("保存失败带原因码与上限（too_large）；只读时不发保存，状态如实写写入闸的原因", async () => {
    const h = harness();
    h.replies.save = [new WorkerError("too_large", 1024)];
    h.type("很长");
    await h.advance(SAVE_IDLE_MS);
    await h.settleAll();
    expect(h.session.getSnapshot().status).toEqual({ kind: "failed", code: "too_large", param: 1024 });
    h.setGate("sync_quota_exceeded");
    h.type("再写");
    await h.advance(SAVE_IDLE_MS);
    await h.settleAll();
    expect(h.saves()).toHaveLength(1);
    expect(h.session.getSnapshot().status).toEqual({ kind: "failed", code: "sync_quota_exceeded" });
    expect(h.session.unsaved).toBe(true);
  });
});

describe("合并与冲突", () => {
  it("已合并：编辑器换成合并结果（在途期间接着打的字不丢），光标随改动映射；撤回 = 以合并那一版为基线存回本方原文", async () => {
    const h = harness();
    h.replies.save = [
      {
        kind: "merged",
        file_id: "f",
        path: "甲.md",
        rev: 5,
        text: "第一段（别处）\n\n第二段，本方",
        undo_text: "第一段\n\n第二段，本方",
      },
    ];
    h.type("，本方");
    void h.session.flush();
    h.type("，接着写");
    h.view.dispatch({ selection: { anchor: h.view.state.doc.length } });
    const cursor = h.view.state.selection.main.head;
    await h.settleAll();
    expect(h.view.state.doc.toString()).toBe("第一段（别处）\n\n第二段，本方，接着写");
    expect(h.view.state.selection.main.head).toBe(cursor + "（别处）".length);
    expect(h.session.baseRev).toBe(5);
    // 撤回换回的是「没有别处那些改动」的本方全文（含在途期间接着打的字）。
    expect(h.session.getSnapshot().merged).toMatchObject({ undoText: "第一段\n\n第二段，本方，接着写" });
    h.type("。");
    expect(h.session.getSnapshot().merged).toBeNull(); // 合并之后又打了字：不再提供撤回

    const clean = harness();
    clean.replies.save = [
      { kind: "merged", file_id: "f", path: "甲.md", rev: 5, text: "别处\n\n第二段，本方", undo_text: "第一段\n\n第二段，本方" },
      { kind: "saved", file_id: "f", path: "甲.md", rev: 6 },
    ];
    clean.type("，本方");
    await clean.advance(SAVE_IDLE_MS);
    await clean.settleAll();
    expect(clean.session.getSnapshot().merged).toMatchObject({ rev: 5, undoText: "第一段\n\n第二段，本方" });
    void clean.session.undoMerge();
    await clean.settleAll();
    expect(clean.view.state.doc.toString()).toBe("第一段\n\n第二段，本方");
    expect(clean.saves()[1]).toMatchObject({ base_rev: 5, text: "第一段\n\n第二段，本方" });
  });

  it("冲突副本：主文件换成别处的版本，记下副本（两份都在）；在途期间接着打的字补进副本", async () => {
    const h = harness();
    h.replies.save = [
      {
        kind: "conflict_copy",
        file_id: "f",
        path: "甲.md",
        rev: 5,
        text: "别处的第一段\n\n第二段",
        copy: { file_id: "c", path: "甲 (冲突 网页 2026-11-06).md", rev: 1 },
      },
      { kind: "saved", file_id: "c", path: "甲 (冲突 网页 2026-11-06).md", rev: 2 },
    ];
    h.type("，本方");
    void h.session.flush();
    h.type("，接着写");
    await h.settleAll();
    expect(h.view.state.doc.toString()).toBe("别处的第一段\n\n第二段");
    expect(h.session.getSnapshot().conflictCopy).toMatchObject({ file_id: "c" });
    expect(h.session.unsaved).toBe(false);
    expect(h.saves()[1]).toMatchObject({ file_id: "c", base_rev: 1, text: "第一段\n\n第二段，本方，接着写" });
  });
});

describe("别处改了", () => {
  it("没有没存上的改动：原地更新成别处的版本（最小差量），不出横幅", async () => {
    const h = harness();
    h.replies.head = [{ rev: 4, path: "甲.md" }];
    h.replies.read = [{ file_id: "f", path: "甲.md", rev: 4, text: "第一段\n\n第二段（别处加的）" }];
    h.view.dispatch({ selection: { anchor: 2 } });
    void h.session.checkRemote();
    await h.settleAll();
    expect(h.view.state.doc.toString()).toBe("第一段\n\n第二段（别处加的）");
    expect(h.view.state.selection.main.head).toBe(2);
    expect(h.session.baseRev).toBe(4);
    expect(h.session.getSnapshot().remoteRev).toBeNull();
  });

  it("有没存上的改动：不覆盖，出横幅；清单里的 rev 没变（就是本页自己的提交）什么也不做", async () => {
    const h = harness();
    h.replies.head = [{ rev: 3, path: "甲.md" }, { rev: 4, path: "甲.md" }];
    void h.session.checkRemote();
    await h.settleAll();
    expect(h.calls.filter((c) => c.cmd === "read")).toHaveLength(0);
    h.type("本方");
    void h.session.checkRemote();
    await h.settleAll();
    expect(h.session.getSnapshot().remoteRev).toBe(4);
    expect(h.view.state.doc.toString()).toBe("第一段\n\n第二段本方");
  });

  it("另存我的这份：本方内容交给 save_copy，这一篇换成别处的版本", async () => {
    const h = harness();
    h.replies.save_copy = [{ file_id: "c", path: "甲 (冲突 网页 2026-11-06).md", rev: 1 }];
    h.replies.read = [{ file_id: "f", path: "甲.md", rev: 4, text: "别处" }];
    h.type("本方");
    const copied = h.session.saveMineAsCopy();
    await h.settleAll();
    expect(await copied).toMatchObject({ file_id: "c" });
    expect(h.calls.find((c) => c.cmd === "save_copy")?.args).toMatchObject({ file_id: "f", text: "第一段\n\n第二段本方" });
    expect(h.view.state.doc.toString()).toBe("别处");
    expect(h.session.unsaved).toBe(false);
  });
});

describe("换篇", () => {
  it("取下编辑器之后状态留在会话里：没存上的改动照样能存，正文从 editorDocumentText 出来", async () => {
    const h = harness();
    h.replies.save = [{ kind: "saved", file_id: "f", path: "甲.md", rev: 4 }];
    h.type("没存的");
    h.session.detach();
    expect(h.session.attached).toBe(false);
    void h.session.flush();
    await h.settleAll();
    expect(h.saves()[0].text).toBe("第一段\n\n第二段没存的");
    expect(h.session.unsaved).toBe(false);
  });
});
