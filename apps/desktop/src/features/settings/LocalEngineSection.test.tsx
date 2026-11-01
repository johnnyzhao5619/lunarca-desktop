/**
 * 本地推理引擎状态（ADR-143）的展示守护：不启用 GGUF 的构建里整节不出现；内存预检拒绝的模型要说清
 * 「需要多少、能给多少」（P0-7 之后的防线是启动前拒绝，用户得知道为什么没加载）；
 * 重启按钮只在熔断时出现——别的状态下它要么无事可做，要么会打断正在进行的自动重启。
 */

import { createElement } from "react";
import { describe, expect, it } from "vitest";

import type { LocalEngineStatusDto } from "../../ipc/bindings";
import { LocalEngineView } from "./LocalEngineSection";
import { renderWithSettingsHosts, translateForTest } from "./settings-test-fixture";

const BASE: LocalEngineStatusDto = {
  runtime_present: true,
  gguf_enabled: true,
  engine_build: "b11429",
  state: "stopped",
  failures: 0,
  last_failure: null,
  models: [],
  refused: [],
};

const render = (status: LocalEngineStatusDto) =>
  renderWithSettingsHosts(createElement(LocalEngineView, { status, restarting: false, onRestart: () => {} }));

describe("本地推理引擎状态", () => {
  it("不启用 GGUF 的构建（正式构建在路由切换前）或本平台没有引擎包时，整节不出现", () => {
    expect(render({ ...BASE, gguf_enabled: false, state: "ready" })).toBe("");
    expect(render({ ...BASE, runtime_present: false, state: "ready" })).toBe("");
  });

  it("空闲停止时显示真实状态、导入入口与自动启动提示", () => {
    const html = render(BASE);
    expect(html).toContain(translateForTest("settings:models.local_engine.import_file"));
    expect(html).toContain(translateForTest("settings:models.local_engine.state.stopped"));
    expect(html).toContain(translateForTest("settings:models.local_engine.idle_hint"));
    expect(render({ ...BASE, last_failure: "exited" })).not.toContain(translateForTest("settings:models.local_engine.idle_hint"));
  });

  it("就绪时列出已加载的模型与上下文，缩减过的标明原因", () => {
    const html = render({
      ...BASE,
      state: "ready",
      models: [
        { model_id: "a", name: "Hy-MT2 1.8B (Q8_0)", context_tokens: 8192, kv_type: "f16", degraded: false },
        { model_id: "b", name: "Qwen3.5 4B (Q4_K_M)", context_tokens: 4096, kv_type: "q8_0", degraded: true },
      ],
    });
    expect(html).toContain(translateForTest("settings:models.local_engine.state.ready"));
    expect(html).toContain("Hy-MT2 1.8B (Q8_0)");
    expect(html).toContain(translateForTest("settings:models.local_engine.loaded_degraded", { name: "Qwen3.5 4B (Q4_K_M)", tokens: 4096 }));
    expect(html).not.toContain(translateForTest("settings:models.local_engine.restart"));
  });

  it("内存预检拒绝的模型说清需要多少、能给多少；估不出来的单独说明", () => {
    const html = render({
      ...BASE,
      refused: [
        { model_id: "big", name: "Qwen3.5 4B (Q6_K)", need_bytes: 4 * 1024 ** 3, budget_bytes: 2 * 1024 ** 3 },
        { model_id: "x", name: "Unknown", need_bytes: null, budget_bytes: null },
      ],
    });
    expect(html).toContain(translateForTest("settings:models.local_engine.refused_label"));
    expect(html).toContain("Qwen3.5 4B (Q6_K)");
    expect(html).toContain(translateForTest("settings:models.local_engine.refused_unknown", { name: "Unknown" }));
    expect(html).not.toContain(translateForTest("settings:models.local_engine.idle_hint"));
  });

  it("只有熔断时给「重新启动」", () => {
    const tripped = render({ ...BASE, state: "tripped", last_failure: "exited" });
    expect(tripped).toContain(translateForTest("settings:models.local_engine.restart"));
    expect(tripped).toContain(translateForTest("settings:models.local_engine.failure.exited"));
    const restarting = render({ ...BASE, state: "restarting", failures: 2, last_failure: "exited" });
    expect(restarting).not.toContain(translateForTest("settings:models.local_engine.restart"));
  });
});
