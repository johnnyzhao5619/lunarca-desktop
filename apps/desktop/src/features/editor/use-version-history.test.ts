import { describe, expect, it } from "vitest";
import { toVersionEntries } from "./use-version-history";
import type { SnapshotDto } from "../../ipc/bindings";

function snapshot(id: string, createdMs: string, size: string): SnapshotDto {
  return {
    id,
    hash: `hash-${id}`,
    created_ms: createdMs,
    size,
  };
}

describe("version history projections", () => {
  it("sorts snapshots newest first and parses numeric fields", () => {
    expect(
      toVersionEntries([
        snapshot("older", "1000", "12"),
        snapshot("newer", "3000", "34"),
        snapshot("middle", "2000", "23"),
      ]).map((entry) => [entry.id, entry.createdMs, entry.sizeBytes]),
    ).toEqual([
      ["newer", 3000, 34],
      ["middle", 2000, 23],
      ["older", 1000, 12],
    ]);
  });
});
