// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { fetchNoteCompletions } from "@lunarca/editor-core/completions";
import { desktopEditorHost } from "./editor-host";

beforeEach(resetBindingsDouble);

describe("[[ 补全候选", () => {
  it("按别名命中的写成 [[真名|别名]]，旁注指向的真名与目录", async () => {
    setCommandReply("vaultWikilinkCandidates", {
      status: "ok",
      data: [
        { title: "周会纪要", insert: "周会纪要", alias: null, folder: "项目", path: "项目/周会纪要.md" },
        { title: "周会纪要", insert: "周会纪要", alias: "例会", folder: "项目", path: "项目/周会纪要.md" },
        { title: "根", insert: "根", alias: "别称", folder: "", path: "根.md" },
      ],
    });
    const options = await fetchNoteCompletions(desktopEditorHost, { notePath: () => "日记.md" }, "例");
    expect(options.map((o) => [o.label, o.detail, o.apply])).toEqual([
      ["周会纪要", "项目", "周会纪要"],
      ["例会", "→ 周会纪要 · 项目", "周会纪要|例会"],
      ["别称", "→ 根", "根|别称"],
    ]);
  });
});

describe("桌面的补全端口", () => {
  it("[[ 候选问 Rust 失败时给空表（不弹补全）", async () => {
    setCommandReply("vaultWikilinkCandidates", { status: "error", error: "vault_unavailable" });
    expect(await desktopEditorHost.wikiLinkCandidates("日记.md", "例")).toEqual([]);
  });

  it("# 候选取标签与笔记数；问 Rust 失败给 null", async () => {
    setCommandReply("tagSummaries", {
      status: "ok",
      data: { tags: [{ tag: "项目", direct_count: 1, note_count: 3, ai_only: false, children: [] }] },
    });
    expect(await desktopEditorHost.tagCounts()).toEqual([{ tag: "项目", noteCount: 3 }]);
    setCommandReply("tagSummaries", { status: "error", error: "vault_unavailable" });
    expect(await desktopEditorHost.tagCounts()).toBeNull();
  });
});
