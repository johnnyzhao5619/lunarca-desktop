// @vitest-environment happy-dom
/**
 * 快速切换器 / 命令面板的搜索框：输入法上屏那一下的回车不是「选中这一行」（PRD-07-040 ①）。
 *
 * WKWebView 是 `compositionend` 先于 `keydown`，上屏的回车 `isComposing = false`——此前
 * `FilteredList` 只判 `isComposing`，于是一按回车上屏就打开了高亮行。
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";
import { useRef } from "react";
import { FilteredList } from "./FilteredList";
import { COMPOSITION_COMMIT_WINDOW_MS } from "../../lib/ime-composition";

afterEach(cleanup);

const QUERY_LABEL = "query";

function at<T extends Event>(event: T, timeStamp: number): T {
  Object.defineProperty(event, "timeStamp", { value: timeStamp });
  return event;
}

function Harness({ onActivate, onCancel }: { onActivate: (item: string) => void; onCancel: () => void }) {
  const inputRef = useRef<HTMLInputElement>(null);
  return (
    <>
      <input ref={inputRef} aria-label={QUERY_LABEL} />
      <FilteredList
        items={["会议纪要", "周报"]}
        query=""
        getKey={(item) => item}
        getText={(item) => item}
        renderRow={(row) => row.item}
        onActivate={onActivate}
        onCancel={onCancel}
        inputRef={inputRef}
      />
    </>
  );
}

function setup() {
  const onActivate = vi.fn();
  const onCancel = vi.fn();
  const view = render(<Harness onActivate={onActivate} onCancel={onCancel} />);
  const input = view.getByLabelText(QUERY_LABEL) as HTMLInputElement;
  input.focus();
  const press = (key: string, timeStamp: number) =>
    input.dispatchEvent(at(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }), timeStamp));
  const commit = (timeStamp: number) => {
    input.dispatchEvent(at(new CompositionEvent("compositionstart", { bubbles: true }), timeStamp - 50));
    input.dispatchEvent(at(new CompositionEvent("compositionend", { bubbles: true }), timeStamp));
  };
  return { onActivate, onCancel, press, commit };
}

describe("FilteredList 搜索框 × 输入法", () => {
  it("上屏那一下的回车不打开高亮行，也不被 preventDefault", () => {
    const { onActivate, press, commit } = setup();
    commit(10_000);
    const notCancelled = press("Enter", 10_000 + COMPOSITION_COMMIT_WINDOW_MS);
    expect(onActivate).not.toHaveBeenCalled();
    expect(notCancelled).toBe(true);
  });

  it("上屏之后再按一次回车照常打开", () => {
    const { onActivate, press, commit } = setup();
    commit(20_000);
    press("Enter", 20_000 + COMPOSITION_COMMIT_WINDOW_MS + 200);
    expect(onActivate).toHaveBeenCalledWith("会议纪要", { split: false });
  });

  it("取消候选的 Esc 不关面板", () => {
    const { onCancel, press, commit } = setup();
    commit(30_000);
    press("Escape", 30_000 + 5);
    expect(onCancel).not.toHaveBeenCalled();
  });
});
