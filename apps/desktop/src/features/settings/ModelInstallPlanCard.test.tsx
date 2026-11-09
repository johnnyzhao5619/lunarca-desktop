/**
 * 安装确认卡的下载量：续传时写「还需下载 / 已下」，与旁边「继续下载（已下 …）」对得上
 * （而不是完整大小）。数字由 Rust 计划算好，这里只守护渲染哪一行。
 */

import { createElement } from "react";
import { describe, expect, it } from "vitest";

import type { ModelInstallPlanDto } from "../../ipc/bindings";
import { formatBytes } from "@lunarca/editor-core/format-bytes";
import { ModelInstallPlanCard } from "./ModelInstallPlanCard";
import { renderWithSettingsHosts, translateForTest } from "./settings-test-fixture";

const BASE: ModelInstallPlanDto = {
  plan_id: "plan",
  model_id: "hy-mt2-1.8b-q4_k_m-gguf",
  name: "Hy-MT2 1.8B (Q4_K_M)",
  download_bytes: 1_133_080_448,
  resumed_bytes: null,
  estimated_on_disk_bytes: 1_133_080_448,
  available_bytes: null,
  remaining_after_bytes: null,
  license_label: "Apache-2.0",
  strict_local_blocked: false,
  can_start: true,
  blocking_reason: null,
  runtime_digest: null,
  replaces_installed: false,
  requires_reindex: false,
};

const render = (plan: ModelInstallPlanDto) =>
  renderWithSettingsHosts(createElement(ModelInstallPlanCard, { plan, onConfirm: () => {}, onCancel: () => {} }));

const bytes = (value: number) => formatBytes(value, "zh-CN");

describe("安装确认卡的下载量", () => {
  it("从头下载写完整下载量", () => {
    const html = render(BASE);
    expect(html).toContain(translateForTest("settings:models.plan.download", { value: bytes(1_133_080_448) }));
  });

  it("续传写还需下载与已下，不再写完整大小", () => {
    const html = render({ ...BASE, download_bytes: 660_091_704, resumed_bytes: 472_988_744 });
    expect(html).toContain(
      translateForTest("settings:models.plan.download_resumed", {
        value: bytes(660_091_704),
        done: bytes(472_988_744),
      }),
    );
    expect(html).not.toContain(translateForTest("settings:models.plan.download", { value: bytes(1_133_080_448) }));
  });
});
