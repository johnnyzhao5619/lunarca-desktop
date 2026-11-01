// @vitest-environment happy-dom
/**
 * 启动概览「快速开始」= 唯一创建动作目录的一个**子集**（PRD-07-068 / TDD-05 §7.5a）。
 *
 * 概览此前自持一份私有 `QUICK_ACTIONS`，于是"有哪些添加方式"在应用里有两个答案——而它们
 * 从第一天起就不一样：那份表里有实时字幕和翻译（它们根本不创建任何东西，按 PRD-07-003a
 * 归顶栏工具区），却没有导入资料。
 *
 * **判据是渲染出来的那几张卡，不是源码里有没有某个词**（批次 E）。此前这一族写成
 * `expect(DASHBOARD).not.toContain("QUICK_ACTIONS")` 之类的残留扫描：它拦得住"名字回来了"，
 * 拦不住"名字没回来但概览又渲染了一个注册表里没有的入口"，也拦不住"用户配置了两项、
 * 概览却画了四项"。现在概览真的挂起来、设置真的下发、卡片真的被数。
 *
 * 本文件归 `features/dashboard/`（而不是隔壁 `capture/`）是**结构约束**：概览依赖捕获域
 * （`useCreateActionStates` / `CREATE_ACTIONS`），反过来引一次就是 `capture ↔ dashboard`
 * 的循环依赖，`check-frontend-structure.mjs` 当场判红。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { CREATE_ACTIONS } from "../capture/create-actions";
import { DashboardView } from "./DashboardView";

/** 与 Rust `luna_config::CREATE_ACTION_IDS` 逐字相同。 */
const RUST_IDS = [
  "text_quick_note",
  "voice_quick_note",
  "new_note",
  "start_recording",
  "import_material",
] as const;

/** 五条能力桩：记录被调到的那一条，并如实回一个终态。 */
function stubContext(trace: string[]) {
  const hit = (name: string) => async () => {
    trace.push(name);
    return "arrived" as const;
  };
  return {
    captureText: hit("captureText"),
    captureVoice: hit("captureVoice"),
    createNote: hit("createNote"),
    startRecording: hit("startRecording"),
    importMaterial: hit("importMaterial"),
  };
}

/** 一份"五条全可用"的动作状态快照。 */
const ALL_AVAILABLE = {
  revision: 1,
  actions: RUST_IDS.map((id) => ({ id, state: { kind: "available" } })),
};

afterEach(cleanup);
beforeEach(resetBindingsDouble);

/** 概览首屏还要问用量/模型/近期记录——这些不是本文件的判据，给出能渲染的最小回答。 */
function stubDashboardIpc(quickStart: string[]) {
  setCommandReply("listSettings", {
    status: "ok",
    data: [
      {
        key: "workspace.quick_start_actions",
        current_value: { kind: "string_list", value: quickStart },
      },
    ],
  });
  setCommandReply("modelManagementSnapshot", {
    status: "ok",
    data: { models: [], assignments: [], storage: null, services: [], remote_compute: null },
  });
  setCommandReply("listUsage", []);
  setCommandReply("usageSummary", null);
  setCommandReply("search", { status: "ok", data: [] });
  setCommandReply("computeStatus", { status: "error", error: "stub" });
  setCommandReply("knowledgePulse", {
    distill: { pending: 0, edited: 0 },
    topics: { stale: 0 },
    ingest: { pending: 0 },
  });
  setCommandReply("checkupReport", { status: "ok", data: { findings: [], truncated: 0 } });
  setCommandReply("createActionStates", { status: "ok", data: ALL_AVAILABLE });
  // 概览自己订处理队列（只取在跑的条数）；队列快照是普通数组，不是 Result 形状。
  setCommandReply("processingQueueSnapshot", []);
}

async function renderDashboard(quickStart: string[], trace: string[] = []) {
  stubDashboardIpc(quickStart);
  const view = render(
    withI18n(createElement(DashboardView, { createActions: stubContext(trace) })),
  );
  await waitFor(() =>
    expect(view.container.querySelector(".luna-dashboard-actions__grid")).not.toBeNull(),
  );
  return view;
}

/** 创建区里每张卡的动作名（卡片结构：图标 / 动作名 / 产物去向）。 */
function actionLabels(view: { container: HTMLElement }): string[] {
  return [
    ...view.container.querySelectorAll(".luna-dashboard-action-card > span:nth-child(2)"),
  ].map((node) => node.textContent ?? "");
}

describe("概览「快速开始」只渲染用户配置的那个子集", () => {
  it("渲染的就是用户配置的那几项、按用户配置的顺序", async () => {
    const view = await renderDashboard(["import_material", "new_note"]);
    await waitFor(() =>
      expect(actionLabels(view)).toEqual([
        t("workspace:create.importMaterial"),
        t("workspace:create.newNote"),
      ]),
    );
  });

  it("前端不写第二份默认值：读不到配置就是空，不兜底成四项", async () => {
    // Rust 已经给过默认值了；前端再写一份，两处默认迟早不一样。
    const view = await renderDashboard([]);
    expect(actionLabels(view)).toEqual([]);
  });

  it("这个版本不认识的 id 被丢掉，而不是渲染时崩掉", async () => {
    // 设置里可能留着一个这个版本已经不认识的动作（降级、手改配置）。
    const view = await renderDashboard(["new_note", "spike_show_popup"]);
    await waitFor(() => expect(actionLabels(view)).toEqual([t("workspace:create.newNote")]));
  });

  it("与「添加内容」消费同一份注册表：文案、去向、动线逐条相同", async () => {
    const trace: string[] = [];
    const view = await renderDashboard(["start_recording"], trace);
    await waitFor(() => expect(actionLabels(view)).toHaveLength(1));
    expect(view.container.textContent).toContain(
      t(`workspace:${CREATE_ACTIONS.start_recording.destinationKey}`),
    );
    fireEvent.click(view.container.querySelector(".luna-dashboard-action-card")!);
    expect(trace).toEqual(["startRecording"]);
  });

  it("创建区里一条都不多——实时字幕与翻译不创建任何东西（PRD-07-003a）", async () => {
    // 退役的 `QUICK_ACTIONS` 里正是它们俩占着两格。判据不是"源码里没有 `captionShowHud`
    // 这个词"，而是"创建区渲染出来的就是注册表那五条，一条不多"。
    const view = await renderDashboard([...RUST_IDS]);
    await waitFor(() => expect(actionLabels(view)).toHaveLength(RUST_IDS.length));
    expect(new Set(actionLabels(view))).toEqual(
      new Set(RUST_IDS.map((id) => t(`workspace:${CREATE_ACTIONS[id].labelKey}`))),
    );
  });

  it("暂时不可用的动作灰掉并就地说明原因（可用性只来自 Rust）", async () => {
    stubDashboardIpc(["new_note"]);
    setCommandReply("createActionStates", {
      status: "ok",
      data: {
        revision: 1,
        actions: [
          {
            id: "new_note",
            state: {
              kind: "needs_attention",
              reason: "vault_unavailable",
              repair: {
                kind: "open_settings",
                route: "settings://notes/storage.vault_root",
              },
            },
          },
        ],
      },
    });
    const view = render(
      withI18n(createElement(DashboardView, { createActions: stubContext([]) })),
    );
    const card = () =>
      view.container.querySelector<HTMLButtonElement>(".luna-dashboard-action-card");
    await waitFor(() => expect(card()?.disabled).toBe(true));
    // 卡片与"为什么不能用 / 怎么解决"是**兄弟**：禁用的按钮不可聚焦，把原因塞进它的
    // `title` 等于键盘与读屏用户根本拿不到。
    expect(view.container.querySelector(".luna-dashboard-action-cell__reason")?.textContent).toContain(
      t("workspace:create.reason.vaultUnavailable"),
    );
    const repair = view.container.querySelector<HTMLButtonElement>(
      ".luna-dashboard-action-cell__repair",
    );
    expect(repair?.disabled, "修复钮必须可独立聚焦").toBe(false);
  });
});
