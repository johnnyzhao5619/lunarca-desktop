import { describe, expect, it } from "vitest";

// 笔记没装载成功（E2 抽样 2026-10-06：库目录无读权限时打开的那篇）：编辑区要说清楚并能重试，
// 不能只剩只读的空白编辑面 +「开始输入…」，状态栏也不能同时报「0 字 · 已保存」。
// VaultEditor 没有渲染级测试夹具，这里按源码锚定三处接线。

const editors = import.meta.glob("./VaultEditor.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;
const workspaces = import.meta.glob("../../app/WorkspaceApp.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const EDITOR = Object.values(editors)[0];
const WORKSPACE = Object.values(workspaces)[0];

describe("笔记装载失败的提示", () => {
  it("扫描面非空", () => {
    expect(EDITOR.length).toBeGreaterThan(200);
    expect(WORKSPACE.length).toBeGreaterThan(200);
  });

  it("失败分支记下错误，提示条带重试且重试会重跑装载", () => {
    const failure = EDITOR.slice(EDITOR.indexOf('if (result.status !== "ok") {'), EDITOR.indexOf("loadFailedRef.current = true;"));
    expect(failure).toContain("setLoadError(message)");
    const notice = EDITOR.match(/\{loadError && path \? \([\s\S]*?\) : null\}/)?.[0] ?? "";
    expect(notice).toContain('role="alert"');
    expect(notice).toContain("setLoadAttempt(");
    expect(EDITOR).toMatch(/\}, \[path, onStatus, applyReveal, loadAttempt\]\);/);
  });

  it("编辑面在失败时隐藏，不显示占位提示", () => {
    const surface = EDITOR.match(/className="luna-editor__surface luna-md-surface"\s+data-hidden=\{([^\n]*)\}/)?.[1] ?? "";
    expect(surface).toContain("loadError !== null");
  });

  it("状态栏在失败时不报字数与保存状态", () => {
    expect(WORKSPACE).toMatch(/activeDocumentPath\.endsWith\("\.md"\)\) && !activeMetrics\.loadFailed && \(/);
  });
});
