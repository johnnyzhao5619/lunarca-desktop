import { describe, expect, it } from "vitest";
import tauriConf from "../../../../src-tauri/tauri.conf.json";

/**
 * CSP 守护（TDD-12 §4.1 / PRD-13 §1.7 架构守卫）。
 *
 * CSP 在 Lunarca 承载的不只是安全加固，还是**商业通道的机制性保证**：计费、配额、用量计量、
 * 隐私路由（严格本地拦截）全部挂在 Rust 侧单一 AI 主干上（ADR-001）——webview 的
 * `connect-src` 不含任何外网源，把「前端偷偷直连云 API、绕过计费与隐私路由」这条旁路
 * 在引擎层焊死。谁想放开它，必须先来改这个测试，并回答"为什么这条流量不走主干"。
 *
 * 逐条断言均已反向验证会红（改 null / 加 https: / 删 object-src 均被抓）。
 */

type TauriConf = {
  app: { security: { csp: string | null; devCsp?: string | null } };
};

function readConf(): TauriConf {
  return tauriConf as unknown as TauriConf;
}

/** 取一条指令的 source 列表（不存在返回 null——有些指令缺失本身就是违规）。 */
function directive(csp: string, name: string): string[] | null {
  const found = csp
    .split(";")
    .map((part) => part.trim())
    .find((part) => part.startsWith(`${name} `) || part === name);
  if (!found) return null;
  return found.slice(name.length).trim().split(/\s+/).filter(Boolean);
}

/**
 * 外网 source 判定：`https:`/`http:` 裸 scheme 通配、或指向非本机 host 的 http(s) 源。
 * 本机豁免：Tauri 的 `http://asset.localhost` / `http://ipc.localhost`、媒体服务的
 * `http://127.0.0.1:*`、dev 的 `ws://localhost:*`。
 */
function isExternalSource(source: string): boolean {
  if (source === "https:" || source === "http:") return true;
  const match = /^(?:https?|wss?):\/\/([^/:]+)/.exec(source);
  if (!match) return false;
  const host = match[1];
  return !["localhost", "127.0.0.1", "asset.localhost", "ipc.localhost"].includes(host);
}

describe("CSP 基线（TDD-12 §4.1）", () => {
  const conf = readConf();

  it("生产 csp 在场（null = 零策略，即回到未设防的现状）", () => {
    expect(conf.app.security.csp).toBeTypeOf("string");
    expect(conf.app.security.csp).not.toHaveLength(0);
  });

  it("script-src 不含 unsafe-eval / unsafe-inline（生产）", () => {
    // Tauri 会为打包产物自动注入 nonce，'self' 之外不需要任何放宽；
    // unsafe-inline 只允许出现在 devCsp（Vite react 插件注入内联 preamble）。
    const scripts = directive(conf.app.security.csp as string, "script-src");
    expect(scripts).not.toBeNull();
    expect(scripts).not.toContain("'unsafe-eval'");
    expect(scripts).not.toContain("'unsafe-inline'");
  });

  it("任何指令都不含外网源——一切外发必须走 Rust 主干（ADR-001 / PRD-13 §1.7）", () => {
    // 这条同时锁死两件事：默认离线（webview 不悄悄加载远程资源）与 CSS 外泄通道
    // （exfiltration 依赖 url() 向外域发请求，img-src/connect-src 无外域即通道死）。
    for (const cspText of [conf.app.security.csp, conf.app.security.devCsp]) {
      if (typeof cspText !== "string") continue;
      for (const part of cspText.split(";")) {
        const sources = part.trim().split(/\s+/).slice(1);
        for (const source of sources) {
          expect(isExternalSource(source), `外网源混入：${source}（${part.trim()}）`).toBe(
            false,
          );
        }
      }
    }
  });

  it("object-src 'none' 与 frame-src 'none' 在场", () => {
    const csp = conf.app.security.csp as string;
    expect(directive(csp, "object-src")).toEqual(["'none'"]);
    expect(directive(csp, "frame-src")).toEqual(["'none'"]);
  });

  it("媒体服务与 asset 协议的必要 source 在场（防'顺手收太紧'弄坏回放）", () => {
    const csp = conf.app.security.csp as string;
    // 媒体服务端口随机（media_server.rs 绑 127.0.0.1:0），必须通配端口。
    expect(directive(csp, "media-src")).toContain("http://127.0.0.1:*");
    // asset 协议双平台形态：macOS/Linux 用 asset:，Windows 用 http://asset.localhost。
    const img = directive(csp, "img-src");
    expect(img).toContain("asset:");
    expect(img).toContain("http://asset.localhost");
  });
});
