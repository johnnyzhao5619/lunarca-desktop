/// <reference types="vite/client" />
/**
 * 批量整理的**拒绝理由必须到得了用户眼前**（R1.7）。
 *
 * 现网缺陷的形状：Rust 侧 `enqueue_distill` 每一篇都会交出一句说得清的拒绝理由，而批量
 * 入口把它整个丢进了一个布尔比较（`enqueue_distill(..) == Ok(true)`）。于是余量把 50 篇
 * 里的 47 篇挡下时，用户拿回的是一句「已加入 3 件」——另外 47 篇既没入队，也没有任何
 * 地方说得出为什么。
 *
 * 这条锁的是**前端这一半**：命令的返回值带着那份清单（生成物即证据），控制器把它交出去，
 * 宿主真的渲染出来。Rust 那一半由 `ipc/distill.rs` 的
 * `the_quota_precheck_runs_before_anything_is_queued` 守（累减 + `Err(reason)` 不被吃掉）。
 */

import { describe, expect, it } from "vitest";
import bindingsSource from "../../ipc/bindings.ts?raw";
import controllerSource from "./use-distill-batch.ts?raw";
import hostSource from "../../app/WorkspaceApp.tsx?raw";
import zhWorkspace from "../../i18n/locales/zh-CN/workspace.json";
import enWorkspace from "../../i18n/locales/en-US/workspace.json";
import zhNotifications from "../../i18n/locales/zh-CN/notifications.json";
import enNotifications from "../../i18n/locales/en-US/notifications.json";

type Dict = Record<string, unknown>;

/** 剥注释：注释里逐字写着这些标识符，带注释扫描的话把实现改坏也不会红。 */
function stripComments(source: string): string {
  return source
    .split("\n")
    .filter((line) => !line.trim().startsWith("*") && !line.trim().startsWith("//"))
    .map((line) => line.split("//")[0] ?? "")
    .join("\n");
}

const controller = stripComments(controllerSource);
const host = stripComments(hostSource);

describe("批量整理：被挡下的篇目说得出是哪些、为什么（R1.7）", () => {
  it("命令返回的不是一个计数，而是「进了几件 + 哪些没进 + 为什么」", () => {
    // bindings.ts 由 export-bindings 生成、CI 重导出后 `git diff --exit-code`，
    // 故它与 Rust 结构恒等——这一格在这里，就是 Rust 真的在下发它。
    expect(
      bindingsSource,
      "distillBatchStart 又退回只回一个数字了——`Err(理由)` 无处可去",
    ).toContain("typedError<DistillBatchStartDto, string>");
    const dto = bindingsSource.slice(
      bindingsSource.indexOf("export type DistillBatchStartDto = {"),
      bindingsSource.indexOf("export type DistillOptionsDto"),
    );
    expect(dto).toContain("queued: number");
    expect(dto).toContain("rejected: DistillBatchRejectionDto[]");
    expect(bindingsSource).toContain("note_rel: string");
  });

  it("控制器把整份结果交出去，不是只留下 queued", () => {
    expect(controller, "控制器又把 rejected 丢掉了").toContain(
      "outcome: DistillBatchStartDto | null",
    );
    // 「已加入 N 件」回执自 2026-09-07 起由 Rust 发 toast（`notify_batch_queued`）；控制器
    // 只保留**有篇目被挡下**的结果——它们才需要一张留得住的卡逐篇说明。
    expect(controller).toContain("setOutcome(result.data.rejected.length > 0 ? result.data : null)");
    expect(controller, "又把「已加入 1 件」做回了常驻条").not.toContain("setOutcome({ queued: 1");
  });

  it("宿主真的渲染每一篇的理由（是哪一篇 + 为什么），而不是只报一个数", () => {
    expect(host).toContain("distillBatch.outcome.rejected.map");
    // 是哪一篇：路径要落到可读的展示名上。
    expect(host, "没说是哪一篇").toContain("noteStem(item.note_rel)");
    // 为什么：Rust 已本地化的说明，剥掉给机器看的前缀后原样呈现。
    expect(host, "没说为什么").toContain("withoutErrorCode(item.reason)");
  });

  it("中英文案都在场（缺一句时界面直接显示裸 key）", () => {
    for (const bundle of [zhWorkspace, enWorkspace] as unknown as Dict[]) {
      const distill = bundle.distill as Dict;
      expect(distill.batchRejected, "缺 distill.batchRejected").toBeTruthy();
      expect(
        String(distill.batchRejected),
        "「另有 N 篇没能加入」必须带上数字，否则它与「已加入 N 件」一样只是一句安慰",
      ).toContain("{{count}}");
    }
    // 「已加入 N 件」现在是通知 toast（PRD-06-051 ⑩ 修订）：标题带数字、动作直达队列。
    for (const bundle of [zhNotifications, enNotifications] as unknown as Dict[]) {
      const queued = bundle.distill_batch_queued as Dict;
      expect(queued, "缺 notifications.distill_batch_queued").toBeTruthy();
      expect(String(queued.title)).toContain("{{count}}");
      expect(queued.action, "toast 没有「查看队列」动作").toBeTruthy();
    }
  });
});
