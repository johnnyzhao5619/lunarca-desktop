/**
 * `availability` 两态在设置面板上各自有**如实说明 + 可行动出口**（TDD-12 §2.1
 * 「schema 代际、可用性出口与回退语义」）。
 *
 * **测试等级要写明**：仓库此刻没有 DOM 环境（happy-dom 随后续批次引入），故这里走
 * `renderToStaticMarkup` 的既有夹具，只覆盖**首帧结构**——effect、事件与点击都不跑。
 * 它能证明"两态各说了什么、各给了哪颗按钮"，**证明不了**"点下去真的调了清除"。后者由
 * 下面那条源码断言（出口按钮不得按行数灰掉）与 Rust 侧的清除用例分别守住。
 */

import { describe, expect, it } from "vitest";
import { WorkspaceMetricsUnavailableNotice } from "./LocalWorkspaceMetricsPanel";
import { renderWithSettingsHosts, translateForTest } from "./settings-test-fixture";

const panels = import.meta.glob("./LocalWorkspaceMetricsPanel.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const PANEL = Object.values(panels)[0] ?? "";

function render(availability: Parameters<typeof WorkspaceMetricsUnavailableNotice>[0]["availability"]) {
  return renderWithSettingsHosts(
    <WorkspaceMetricsUnavailableNotice
      availability={availability}
      onRetry={() => {}}
      onClear={() => {}}
    />,
  );
}

describe("读不到观测时的可行动出口", () => {
  it("扫描面非空", () => {
    expect(PANEL.length).toBeGreaterThan(1000);
  });

  it("能读的时候什么都不说——一句多余的状态行本身就是噪音", () => {
    expect(render({ kind: "ready" })).toBe("");
    expect(render({ kind: "empty" })).toBe("");
  });

  it("`storage_unavailable` 说清「此刻记不了也读不了」并给重试", () => {
    const markup = render({ kind: "unavailable", code: "storage_unavailable" });
    expect(markup).toContain(
      translateForTest("settings:workspace_metrics.unavailable.storageUnavailable"),
    );
    expect(markup).toContain(translateForTest("settings:workspace_metrics.unavailable.retry"));
    // 这一态不该把用户推去清除：他的数据还在，只是此刻拿不到。
    expect(markup).not.toContain(
      translateForTest("settings:workspace_metrics.unavailable.clearAndRebuild"),
    );
  });

  it("`schema_invalid` 必须把用户指向显式清除——那是 fail closed 之后唯一的恢复路径", () => {
    const markup = render({ kind: "unavailable", code: "schema_invalid" });
    expect(markup).toContain(
      translateForTest("settings:workspace_metrics.unavailable.schemaInvalid"),
    );
    expect(markup).toContain(
      translateForTest("settings:workspace_metrics.unavailable.clearAndRebuild"),
    );
    // 不给出口就是让用户对着一个永远空的面板无从下手，而"我们没删你的东西"这句话
    // 反而成了牢门。
    expect(markup).not.toContain(translateForTest("settings:workspace_metrics.unavailable.retry"));
  });

  it("出口按钮不得按行数灰掉：坏 schema 下一行都没有，而清除正是唯一那条路", () => {
    // 首帧结构证明不了 `disabled` 的判据（这颗按钮在面板本体里，而面板要一次 IPC 往返
    // 才有数据）。判据本身只能由结构看住——它此前恰好写成 `rows.length === 0`。
    expect(PANEL).toContain("disabled={rows.length === 0 && !unavailable}");
  });

  it("面板挂载 / 存储状态变化 / 重新聚焦都重查摘要（§2.1 末条）", () => {
    expect(PANEL).toContain("events.workspaceMetricsAvailability.listen");
    expect(PANEL).toContain('window.addEventListener("focus", refresh)');
    expect(PANEL).toContain('window.removeEventListener("focus", refresh)');
  });
});
