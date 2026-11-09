#!/usr/bin/env node
// 网页版的本地静态预览（TDD-17 §6.1 / §6.3）：提供 `vite build` 的产物，带与线上入口站点相同的全套响应头；
// `/api/*` 去掉前缀转发给服务端，与页面同源（refresh Cookie 是 `__Host-` 前缀，刷新接口必须同源）。
//
// 用法：LUNARCA_API=http://127.0.0.1:8080 PORT=4173 node scripts/preview-server.mjs
// 线上由 vps-edge 的入口站点按同一份响应头与白名单提供；这里只给本地验证与端到端用。
import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer, request as httpRequest } from "node:http";
import { extname, join, normalize, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const webRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const dist = join(webRoot, "dist");
const api = new URL(process.env.LUNARCA_API ?? "http://127.0.0.1:8080");
const port = Number.parseInt(process.env.PORT ?? "4173", 10);
const host = process.env.HOST ?? "127.0.0.1";

const COMMON_HEADERS = {
  "Strict-Transport-Security": "max-age=31536000",
  "X-Content-Type-Options": "nosniff",
  "Referrer-Policy": "no-referrer",
  "Permissions-Policy": "camera=(), microphone=(), geolocation=()",
  "Cross-Origin-Opener-Policy": "same-origin",
};

/** TDD-17 §6.1 的全套响应头；违规报告送回服务端计数。 */
export const SECURITY_HEADERS = {
  "Content-Security-Policy": [
    "default-src 'none'",
    "script-src 'self' 'wasm-unsafe-eval'",
    "style-src 'self'",
    "img-src 'self' blob:",
    "media-src 'none'",
    "connect-src 'self'",
    "worker-src 'self'",
    "font-src 'self'",
    "frame-src 'self'",
    "frame-ancestors 'none'",
    "base-uri 'none'",
    "form-action 'self'",
    "object-src 'none'",
    "require-trusted-types-for 'script'",
    "trusted-types lunarca",
    "report-uri /api/v1/web/csp-report",
  ].join("; "),
  ...COMMON_HEADERS,
};

export const SANDBOX_PAGE = "/render-sandbox.html";

/**
 * 图表渲染沙箱页（TDD-17 §6.1）自己的一份 CSP：不开 Trusted Types；Mermaid 往 SVG 里写 `<style>` 与
 * `style` 属性，样式放行 `'unsafe-inline'`（只这一页）；一个请求都不许发；只许本站页面嵌。
 * `sandbox allow-scripts` 让它即使被直接打开也是不透明来源。违规不上报：不透明来源发不出请求。
 */
export const SANDBOX_HEADERS = {
  "Content-Security-Policy": [
    "default-src 'none'",
    "script-src 'self'",
    "style-src 'unsafe-inline'",
    "img-src 'none'",
    "connect-src 'none'",
    "font-src 'none'",
    "frame-ancestors 'self'",
    "base-uri 'none'",
    "form-action 'none'",
    "object-src 'none'",
    "sandbox allow-scripts",
  ].join("; "),
  ...COMMON_HEADERS,
};

/**
 * 沙箱页是不透明来源，它取本站的脚本一律是跨源请求（模块脚本与 SRI 都要 CORS）：静态资源放行任意来源的
 * 无凭据读取。这些都是公开的构建产物；`/api/*` 不带这一项。
 */
const ASSET_HEADERS = { "Access-Control-Allow-Origin": "*" };

/** 入口白名单（TDD-17 §6.3）：只有这几组转发给服务端，其余 `/api/` 一律 404。 */
const API_ALLOW = [/^\/v1\/web\//, /^\/v1\/account\/web-access$/, /^\/v1\/entitlements$/, /^\/sync\/v1\/vaults(?:\/|$|\?)/];

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".wasm": "application/wasm",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".woff2": "font/woff2",
};

function proxy(req, res, path) {
  const upstream = httpRequest(
    {
      protocol: api.protocol,
      hostname: api.hostname,
      port: api.port,
      method: req.method,
      path,
      headers: { ...req.headers, host: api.host },
    },
    (upstreamRes) => {
      res.writeHead(upstreamRes.statusCode ?? 502, upstreamRes.headers);
      upstreamRes.pipe(res);
    },
  );
  upstream.on("error", () => {
    if (!res.headersSent) res.writeHead(502, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: "upstream_unavailable" }));
  });
  req.pipe(upstream);
}

function serveFile(res, file) {
  const isSandbox = file === join(dist, SANDBOX_PAGE);
  const isEntry = isSandbox || file.endsWith(`${sep}index.html`);
  res.writeHead(200, {
    ...(isSandbox ? SANDBOX_HEADERS : SECURITY_HEADERS),
    ...(file.startsWith(join(dist, "assets") + sep) ? ASSET_HEADERS : {}),
    "content-type": TYPES[extname(file)] ?? "application/octet-stream",
    "cache-control": isEntry ? "no-store" : "public, max-age=31536000, immutable",
  });
  createReadStream(file).pipe(res);
}

export const server = createServer((req, res) => {
  const url = new URL(req.url ?? "/", "http://local");
  if (url.pathname.startsWith("/api/")) {
    const rest = url.pathname.slice(4);
    if (!API_ALLOW.some((rule) => rule.test(rest)) || rest.startsWith("/sync/v1/admin")) {
      res.writeHead(404, { ...SECURITY_HEADERS, "content-type": "application/json" });
      res.end(JSON.stringify({ error: "not_found" }));
      return;
    }
    proxy(req, res, `${rest}${url.search}`);
    return;
  }
  const candidate = normalize(join(dist, decodeURIComponent(url.pathname)));
  if (candidate.startsWith(dist) && existsSync(candidate) && statSync(candidate).isFile()) {
    serveFile(res, candidate);
    return;
  }
  // SPA：其余路径回入口 HTML。
  serveFile(res, join(dist, "index.html"));
});

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  server.listen(port, host, () => console.log(`网页预览：http://${host}:${port} → API ${api.origin}`));
}
