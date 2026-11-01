import { describe, expect, it } from "vitest";
import type { KnowledgeReceiptDto } from "../../ipc/bindings";
import { upsertKnowledgeReceipt } from "./knowledge-receipts";

function receipt(operation: string, revision: number): KnowledgeReceiptDto {
  return {
    receipt_id: `${operation}-${revision}`,
    operation_id: operation,
    revision,
    kind: "distill",
    outcome: revision === 1 ? "queued" : "updated",
    primary_note: { path: "Notes/A.md" },
    updated_topics: [],
    queued_steps: [],
    degraded_steps: [],
    primary_action: "open_note",
    applied_items: null,
  };
}

describe("knowledge receipt revisions", () => {
  it("replaces an operation with its newer revision and ignores duplicates/stale delivery", () => {
    const queued = receipt("op", 1);
    const done = receipt("op", 2);
    expect(upsertKnowledgeReceipt([queued], done)).toEqual([done]);
    expect(upsertKnowledgeReceipt([done], queued)).toEqual([done]);
    expect(upsertKnowledgeReceipt([done], done)).toEqual([done]);
  });
});

