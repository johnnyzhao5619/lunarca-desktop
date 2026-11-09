import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import type { Plugin } from "vite";
import { defineConfig } from "vitest/config";

/**
 * 两张入口 HTML：应用本身与图表渲染沙箱页（TDD-17 §6.1，各有一份 CSP，由入口站点按路径给）。
 */
const ENTRY_PAGES = ["index.html", "render-sandbox.html"];

/**
 * 入口 HTML 里的脚本与样式一律带 SRI（TDD-17 §6.1）：构建后按产物内容算 sha384 写进 `integrity`。
 * Worker、WASM 与按需加载的分块不经入口 HTML 加载，由 CSP 的 `'self'` 管。
 */
function subresourceIntegrity(): Plugin {
  return {
    name: "lunarca-sri",
    apply: "build",
    enforce: "post",
    // 在产物落盘之后算：动态 import 的预载占位要到生成阶段末尾才替换，早一步算出来的摘要对不上。
    writeBundle(output) {
      const dir = output.dir ?? "dist";
      for (const page of ENTRY_PAGES) {
        const entry = join(dir, page);
        const html = readFileSync(entry, "utf8").replace(
          /<(script|link)\b([^>]*?)\s(src|href)="\/(assets\/[^"]+)"([^>]*)>/g,
          (_tag, element: string, before: string, attr: string, file: string, after: string) => {
            const digest = createHash("sha384").update(readFileSync(join(dir, file))).digest("base64");
            return `<${element}${before} ${attr}="/${file}" integrity="sha384-${digest}"${after}>`;
          },
        );
        writeFileSync(entry, html);
      }
    },
  };
}

/**
 * CodeMirror 的基础主题进**构造样式表**（`adoptedStyleSheets`），不进 `<style>` 元素。
 *
 * CM6 的基础主题由 `style-mod` 在运行时生成；它只在挂到 Shadow DOM 时用构造样式表，挂在整页文档上时
 * 往 `<head>` 插一个 `<style>` 元素。入口站点的 CSP 是 `style-src 'self'`（TDD-17 §6.1），`<style>` 元素
 * 会被拦下并上报一次违规，编辑器没有基础样式（光标层、选区层、折行全错位）。构造样式表走 CSSOM，不受
 * `style-src` 约束，而整页文档本来就支持它（PRD-21-050 的浏览器范围都有）——所以只把「挂在 Shadow DOM 上」
 * 这个前提去掉，其余行为不变。依赖升级后这一行对不上就让构建失败，不静默退回 `<style>`。
 */
const STYLE_MOD_DOCUMENT_GATE = "if (!root.head && root.adoptedStyleSheets && win.CSSStyleSheet) {";

function codeMirrorStylesWithoutInlineStyle(): Plugin {
  let patched = false;
  return {
    name: "lunarca-style-mod-adopted-sheets",
    apply: "build",
    transform(code, id) {
      if (!/[\\/]style-mod[\\/]src[\\/]style-mod\.js$/.test(id)) return null;
      if (!code.includes(STYLE_MOD_DOCUMENT_GATE)) {
        throw new Error("style-mod 的构造样式表分支变了：核对 CodeMirror 的基础主题在 CSP 下怎么挂（vite.config.ts）");
      }
      patched = true;
      return { code: code.replace(STYLE_MOD_DOCUMENT_GATE, "if (root.adoptedStyleSheets && win.CSSStyleSheet) {"), map: null };
    },
    buildEnd(error) {
      // 打进了 CodeMirror 就必须改到过：没改到 = 编辑器又会往 <head> 插 <style>。
      if (!error && !patched && [...this.getModuleIds()].some((id) => /[\\/]@codemirror[\\/]view[\\/]/.test(id))) {
        throw new Error("没有找到 style-mod 的模块：CodeMirror 的基础主题会以 <style> 元素挂上，被 CSP 拦下（vite.config.ts）");
      }
    },
  };
}

const pkg = JSON.parse(readFileSync(new URL("./package.json", import.meta.url), "utf8")) as { version: string };

export default defineConfig({
  plugins: [react(), codeMirrorStylesWithoutInlineStyle(), subresourceIntegrity()],
  clearScreen: false,
  define: {
    // 页脚的服务端版本：网页与服务端锁步发布（ADR-177 ⑨），由镜像构建时注入；本地构建取包版本。
    __LUNARCA_SERVER_VERSION__: JSON.stringify(process.env.LUNARCA_SERVER_VERSION ?? pkg.version),
  },
  build: {
    outDir: "dist",
    target: "es2022",
    emptyOutDir: true,
    // 不内联任何资源：`data:` 不在 CSP 白名单里（TDD-17 §6.1）。
    assetsInlineLimit: 0,
    rollupOptions: {
      input: Object.fromEntries(ENTRY_PAGES.map((page) => [page.replace(/\.html$/, ""), fileURLToPath(new URL(page, import.meta.url))])),
    },
  },
  worker: {
    format: "es",
  },
  test: {
    css: true,
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
