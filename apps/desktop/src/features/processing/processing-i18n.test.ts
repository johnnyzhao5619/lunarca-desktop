import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import zhWorkspace from "../../i18n/locales/zh-CN/workspace.json";
import enWorkspace from "../../i18n/locales/en-US/workspace.json";
import type { ProcPhase } from "../../ipc/bindings";

/**
 * 队列任务的**动态查表文案**守护。
 *
 * `procKindLabel` / `procRunningLabel` 拼的是 `processing.kind.<tag>` 与
 * `processing.phase.<step>`——两者都是 **Rust 侧字符串**，静态扫描 `t("…")` 的正则看不见。
 * 漏写文案时任务面板上显示的是裸 key（「processing.phase.filing」），而 typecheck /
 * lint / 既有测试一个都不会红。
 *
 * 阶段清单用 `Record<ProcPhase, true>` **由类型系统强制穷尽**：Rust 新增一个
 * `ProcPhase` 变体、绑定再生之后，这里不补一行就编译不过——比"记得回来加一行"可靠。
 * 任务类型（`ProcTaskDto.kind`）在绑定里是裸 `string`（DTO 只投影展示标签），
 * 拿不到同样的强制，故显式列出并在此说明它是 `ProcTaskKind::tag()` 的镜像。
 */

const ALL_PHASES: Record<ProcPhase, true> = {
  fetching: true,
  decoding: true,
  converting: true,
  ingesting: true,
  awaiting_server: true,
  transcribing: true,
  translating: true,
  diarizing: true,
  distilling: true,
  rewriting: true,
  filing: true,
  maintaining_knowledge: true,
};

/**
 * 任务类型清单**从 Rust 源现读**（`processing_queue.rs` 的 `KNOWN_TAGS`）+ 回填认不出时的
 * `history`。此前这里是手抄的镜像，Rust 加了 `knowledge_lifecycle` / `knowledge_apply` 而这里
 * 没跟——真机 2026-10-02：任务面板上一条回填的失败项显示「processing.kind.knowledge_lifecycle」。
 */
const QUEUE_RS = readFileSync(resolve(__dirname, "../../../../../src-tauri/src/processing_queue.rs"), "utf8");
const ALL_KINDS = [
  ...[...(QUEUE_RS.match(/const KNOWN_TAGS: \[&str; \d+\] = \[([\s\S]*?)\];/)?.[1] ?? "").matchAll(/"([a-z_]+)"/g)].map(
    (m) => m[1],
  ),
  "history",
];

type Dict = Record<string, unknown>;

function lookup(bundle: Dict, path: string): string | undefined {
  const value = path.split(".").reduce<unknown>((node, key) => {
    return typeof node === "object" && node !== null ? (node as Dict)[key] : undefined;
  }, bundle);
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

describe("处理队列的动态文案（PRD-05-012 / PRD-16-024）", () => {
  it("每个运行阶段在中英文案里都在场", () => {
    for (const phase of Object.keys(ALL_PHASES)) {
      for (const bundle of [zhWorkspace, enWorkspace] as unknown as Dict[]) {
        expect(lookup(bundle, `processing.phase.${phase}`), `缺 phase.${phase}`).toBeTruthy();
      }
    }
  });

  it("每个任务类型在中英文案里都在场", () => {
    // 现读失败时清单会是空的，下面的循环就什么也不验——先锁住它真的读到了。
    expect(ALL_KINDS).toContain("knowledge_lifecycle");
    expect(ALL_KINDS.length).toBeGreaterThan(10);
    for (const kind of ALL_KINDS) {
      for (const bundle of [zhWorkspace, enWorkspace] as unknown as Dict[]) {
        expect(lookup(bundle, `processing.kind.${kind}`), `缺 kind.${kind}`).toBeTruthy();
      }
    }
  });

  it("阶段名两两不同——同一个词指两件事，用户就说不清「我点的那个跑到哪儿了」", () => {
    // 归入刻意不叫 `ingesting`（已被导入的媒体落库占用）。这条断言把那个裁决钉在文案上：
    // 中文标签撞车即红。
    const zh = Object.keys(ALL_PHASES).map(
      (phase) => lookup(zhWorkspace as unknown as Dict, `processing.phase.${phase}`)!,
    );
    expect(new Set(zh).size).toBe(zh.length);
  });
});
