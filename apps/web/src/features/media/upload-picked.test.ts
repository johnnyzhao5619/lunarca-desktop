/**
 * 「插入附件」选中的文件（PRD-21-025）：与拖入同一条上传路，返回落好的链接；只读与未存成文件时不传。
 */

import { describe, expect, it, vi } from "vitest";
import { WorkerError } from "../../worker/worker-client";
import { uploadPicked, type UploadDeps, type UploadStatus, type UploadTarget } from "./upload-extension";

interface UploadArgs {
  note_path: string;
  name: string;
  kind: string;
}

function deps(upload: (name: string) => Promise<{ link: string }>) {
  const reports: UploadStatus[] = [];
  const call = vi.fn(async (_cmd: string, args: UploadArgs) => ({ file_id: "f", rev: 1, ...(await upload(args.name)) }));
  const report = (status: UploadStatus) => {
    reports.push(status);
  };
  return { reports, call, deps: { call, report } as unknown as UploadDeps };
}

const target = (overrides: Partial<UploadTarget> = {}): UploadTarget => ({
  notePath: () => "笔记/周报.md",
  readOnly: () => null,
  ...overrides,
});

const file = (name: string) => new File(["x"], name, { type: "application/pdf" });

describe("uploadPicked", () => {
  it("逐个上传、按选中的顺序返回链接；上传参数与拖入同为 named", async () => {
    const { deps: d, call, reports } = deps(async (name) => ({ link: `附件/${name}` }));
    expect(await uploadPicked(d, target(), [file("a.pdf"), file("b.pdf")])).toEqual(["附件/a.pdf", "附件/b.pdf"]);
    expect(call.mock.calls.map(([, args]) => args.kind)).toEqual(["named", "named"]);
    expect(call.mock.calls[0][1].note_path).toBe("笔记/周报.md");
    expect(reports.at(-1)).toEqual({ kind: "done" });
  });

  it("只读时不传、返回空，报告写入闸的原因；还没存成文件时提示先保存", async () => {
    const readOnly = deps(async () => ({ link: "x" }));
    expect(await uploadPicked(readOnly.deps, target({ readOnly: () => "sync_read_only" }), [file("a.pdf")])).toEqual([]);
    expect(readOnly.call).not.toHaveBeenCalled();
    expect(readOnly.reports).toEqual([{ kind: "failed", code: "sync_read_only" }]);

    const unsaved = deps(async () => ({ link: "x" }));
    expect(await uploadPicked(unsaved.deps, target({ notePath: () => null }), [file("a.pdf")])).toEqual([]);
    expect(unsaved.reports).toEqual([{ kind: "save_first" }]);
  });

  it("中途失败：报告原因码与参数，已经传好的那几个照样返回", async () => {
    const { deps: d, reports } = deps(async (name) => {
      if (name === "big.pdf") throw new WorkerError("too_large", 1024);
      return { link: `附件/${name}` };
    });
    expect(await uploadPicked(d, target(), [file("a.pdf"), file("big.pdf"), file("c.pdf")])).toEqual(["附件/a.pdf"]);
    expect(reports.at(-1)).toEqual({ kind: "failed", code: "too_large", param: 1024 });
  });
});
