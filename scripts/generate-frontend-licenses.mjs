#!/usr/bin/env node
// 前端第三方许可归示聚合（TDD-04 §5、TDD-13 §2 的前端半边；Rust 侧由 cargo-about 承担）。
//
// 从 apps/desktop/package-lock.json（lockfileVersion >= 2）取**生产依赖闭包**
// （排除 dev 标记条目——发行 .app 只含打进 bundle 的运行时依赖），对每个包聚合
// license 字段与包内许可文本，输出单一 Markdown 归示文件。
//
// 用法：
//   node scripts/generate-frontend-licenses.mjs                  # 输出到 stdout
//   node scripts/generate-frontend-licenses.mjs -o OUT.md        # 写文件（发布管线用，产物不入库）
//   node scripts/generate-frontend-licenses.mjs --check          # CI 门禁：任一生产包许可不可识别即退出码 1
import { readFileSync, existsSync, readdirSync, writeFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const appRoot = join(dirname(fileURLToPath(import.meta.url)), "..", "apps", "desktop");
const lock = JSON.parse(readFileSync(join(appRoot, "package-lock.json"), "utf8"));
if (!lock.packages || lock.lockfileVersion < 2) {
  console.error(`需要 lockfileVersion >= 2 的 package-lock.json（当前：${lock.lockfileVersion}）`);
  process.exit(1);
}

const args = process.argv.slice(2);
const checkMode = args.includes("--check");
const outIdx = args.indexOf("-o");
const outPath = outIdx >= 0 ? args[outIdx + 1] : null;

// 生产闭包：packages 里排除根条目与 dev 标记；嵌套重复（a/node_modules/b）按 name@version 去重。
const entries = new Map();
for (const [path, meta] of Object.entries(lock.packages)) {
  if (!path.startsWith("node_modules/") || meta.dev) continue;
  const name = path.replace(/^.*node_modules\//, "");
  const key = `${name}@${meta.version}`;
  if (!entries.has(key)) entries.set(key, { name, version: meta.version, dir: join(appRoot, path), lockLicense: meta.license });
}

const LICENSE_FILE = /^(licen[cs]e|copying|notice)(\.(md|txt|markdown))?$/i;
const problems = [];
const sections = [];
for (const e of [...entries.values()].sort((a, b) => a.name.localeCompare(b.name))) {
  let license = e.lockLicense;
  const pkgJson = join(e.dir, "package.json");
  if (!license && existsSync(pkgJson)) {
    const parsed = JSON.parse(readFileSync(pkgJson, "utf8")).license;
    license = typeof parsed === "string" ? parsed : parsed?.type;
  }
  let text = null;
  if (existsSync(e.dir)) {
    const file = readdirSync(e.dir).find((n) => LICENSE_FILE.test(n));
    if (file) text = readFileSync(join(e.dir, file), "utf8").trim();
  }
  if (!license && !text) {
    problems.push(`${e.name}@${e.version}：既无 license 字段也无许可文本文件`);
    continue;
  }
  sections.push(
    `## ${e.name} ${e.version} — ${license ?? "见下方许可文本"}\n\n` +
      (text ? "```\n" + text + "\n```" : `（包内未附许可文本文件，SPDX 标识：${license}）`)
  );
}

if (checkMode) {
  if (problems.length) {
    console.error(`前端归示门禁失败，${problems.length} 个生产包许可不可识别：`);
    for (const p of problems) console.error(`  - ${p}`);
    process.exit(1);
  }
  console.log(`前端归示门禁通过：${sections.length} 个生产包许可均可识别并可归示。`);
  process.exit(0);
}

const out =
  `# 前端第三方依赖许可归示（apps/desktop 生产依赖闭包）\n\n` +
  `> 由 scripts/generate-frontend-licenses.mjs 从 package-lock.json 生成；共 ${sections.length} 个包。\n\n` +
  sections.join("\n\n---\n\n") +
  "\n";
if (outPath) {
  writeFileSync(outPath, out);
  console.error(`已写出 ${outPath}（${sections.length} 个包）`);
} else {
  process.stdout.write(out);
}
