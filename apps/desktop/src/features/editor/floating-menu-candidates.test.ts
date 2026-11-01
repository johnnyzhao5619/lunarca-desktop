import { describe, expect, it } from "vitest";
import {
  currentCandidate,
  INITIAL_RESULT,
  pushCandidate,
  updateCandidate,
  type FloatingMenuResult,
} from "./use-floating-menu";

function running(action: FloatingMenuResult["action"] = "optimize"): FloatingMenuResult {
  return { ...INITIAL_RESULT, status: "running", action };
}

describe("floating menu candidates", () => {
  it("starts empty and reports the idle result", () => {
    expect(currentCandidate({ items: [], index: 0 })).toEqual(INITIAL_RESULT);
  });

  it("a fresh action replaces the list; retry and follow-up append and select the newest", () => {
    let list = pushCandidate({ items: [], index: 0 }, running(), true);
    list = updateCandidate(list, 0, (item) => ({ ...item, status: "done", output: "v1" }));
    list = pushCandidate(list, running(), false);
    expect(list.items).toHaveLength(2);
    expect(list.index).toBe(1);
    list = updateCandidate(list, 1, (item) => ({ ...item, status: "done", output: "v2" }));
    expect(list.items[0].output).toBe("v1");
    expect(currentCandidate(list).output).toBe("v2");

    const fresh = pushCandidate(list, running("translate"), true);
    expect(fresh.items).toHaveLength(1);
    expect(fresh.index).toBe(0);
  });

  it("only touches the candidate a stream event belongs to", () => {
    let list = pushCandidate({ items: [], index: 0 }, running(), true);
    list = pushCandidate(list, running(), false);
    list = updateCandidate(list, 0, (item) => ({ ...item, output: "first" }));
    expect(list.items[0].output).toBe("first");
    expect(list.items[1].output).toBe("");
    expect(updateCandidate(list, 5, (item) => item)).toBe(list);
  });
});
