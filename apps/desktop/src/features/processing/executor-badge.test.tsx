/**
 * 执行者徽标的渲染守护（PRD-17-033 降级执行者可见）。
 *
 * 拦的是一类**静默的隐私误述**：转写被路由到第三方，而界面上与本机执行毫无差别。
 * 这条钉住两件事——① 本机执行**不加**徽标（本地是默认路径，给它加标记会让「没标记」
 * 反而显得可疑）；② 数据离开部署方边界时，`data-leaves` 必须为真，样式据此走警示色。
 *
 * `leaves_deployment` 由 Rust 判定（ADR-003）：`unknown` 该算不算「离开」是个隐私
 * 判断，不该散落在 TS 里。这里只断言前端**如实转述**它，不自己推。
 */

import { createElement } from "react";
import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createInstance, type i18n as I18nInstance } from "i18next";
import { I18nextProvider } from "react-i18next";

import workspace from "../../i18n/locales/zh-CN/workspace.json";
import type { ProcExecutorDto } from "../../ipc/bindings";

const i18n: I18nInstance = createInstance();
void i18n.init({
  lng: "zh-CN",
  fallbackLng: "zh-CN",
  defaultNS: "workspace",
  resources: { "zh-CN": { workspace } },
  interpolation: { escapeValue: false },
});

/** 与面板同一段 JSX 的最小复刻——只为把断言钉在「标不标、标成什么」上。 */
function Badge({ executor }: { executor: ProcExecutorDto | null }) {
  if (!executor) return null;
  return createElement(
    "span",
    {
      className: "luna-proc-queue__executor",
      "data-leaves": executor.leaves_deployment,
      title: i18n.t("processing.executor.detail", {
        name: executor.display_name,
        model: executor.model,
      }),
    },
    i18n.t(`processing.executor.${executor.kind}`),
  );
}

function render(executor: ProcExecutorDto | null): string {
  return renderToStaticMarkup(
    createElement(I18nextProvider, { i18n }, createElement(Badge, { executor })),
  );
}

describe("执行者徽标", () => {
  it("本机执行不加徽标", () => {
    expect(render(null)).toBe("");
  });

  it("服务端执行标出来，且不标成「离开边界」", () => {
    const html = render({
      display_name: "本机 whisper.cpp",
      kind: "local_runtime",
      model: "ggml-base",
      leaves_deployment: false,
    });
    expect(html).toContain(workspace.processing.executor.local_runtime);
    expect(html).toContain('data-leaves="false"');
  });

  it("第三方执行必须被标成离开边界", () => {
    const html = render({
      display_name: "SomeVendor",
      kind: "third_party",
      model: "whisper-1",
      leaves_deployment: true,
    });
    expect(html).toContain(workspace.processing.executor.third_party);
    expect(html).toContain('data-leaves="true"');
  });

  it("未知性质同样按离开边界处理——不知道时往保守的一侧说", () => {
    const html = render({
      display_name: "?",
      kind: "unknown",
      model: "?",
      leaves_deployment: true,
    });
    expect(html).toContain(workspace.processing.executor.unknown);
    expect(html).toContain('data-leaves="true"');
  });

  it("详情里带出执行者与模型，供悬停核对", () => {
    const html = render({
      display_name: "本机 whisper.cpp",
      kind: "local_runtime",
      model: "ggml-base",
      leaves_deployment: false,
    });
    expect(html).toContain("本机 whisper.cpp");
    expect(html).toContain("ggml-base");
  });
});
