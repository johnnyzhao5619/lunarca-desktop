import { describe, expect, it } from "vitest";

import { resolveChatRouteLabel } from "./ChatView";

const t = (key: string) => `translated:${key}`;

describe("AI 对话执行路径文案", () => {
  it("服务端托管时不把空本机目录误报成无可用模型", () => {
    expect(resolveChatRouteLabel("managed_service", undefined, true, t)).toBe(
      "translated:route.managed_service",
    );
  });

  it("本机未配置时如实说没配", () => {
    expect(resolveChatRouteLabel("on_device", undefined, true, t)).toBe(
      "translated:route.local_unconfigured",
    );
  });

  it("本机档优先展示实际生效的本机模型名", () => {
    expect(resolveChatRouteLabel("on_device", "Qwen3.5 4B", true, t)).toBe("Qwen3.5 4B");
  });

  it("首帧与状态读取失败均不猜成本机缺模型", () => {
    expect(resolveChatRouteLabel(null, undefined, false, t)).toBe("translated:route.loading");
    expect(resolveChatRouteLabel(null, undefined, true, t)).toBe("translated:route.unavailable");
  });
});
