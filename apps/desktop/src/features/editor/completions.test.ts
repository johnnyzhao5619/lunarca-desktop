// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { fetchNoteCompletions } from "./completions";

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
    const options = await fetchNoteCompletions({ notePath: () => "日记.md" }, "例");
    expect(options.map((o) => [o.label, o.detail, o.apply])).toEqual([
      ["周会纪要", "项目", "周会纪要"],
      ["例会", "→ 周会纪要 · 项目", "周会纪要|例会"],
      ["别称", "→ 根", "根|别称"],
    ]);
  });
});
