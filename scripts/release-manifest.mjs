// 发布清单与更新包验签（客户端分发 D3，TDD-13 §5）：`scripts/release-macos.sh` 打完更新包后调用。
//
// 两件事，都在发出去**之前**拦：
// 1. 更新包的 minisign 签名必须能被编进应用的那把公钥（tauri.conf.json `plugins.updater.pubkey`）
//    验过。私钥与公钥不是一对时，tauri signer 照样签得出来、发布照样成功，但**每一个用户**点
//    「下载并安装」都会验签失败——而旧版本里的公钥收不回，这一版就永远升不上去。
// 2. latest.json 的形状对：版本号是语义化版本、地址落在这一版自己的发布目录、签名就是 .sig 原文。
//
// minisign 格式（tauri signer 的产物，`.sig` 与公钥串都是「整个文本文件」再 base64 一次）：
//   公钥文本第 2 行 base64 → "Ed"(2) + key id(8) + Ed25519 公钥(32)
//   签名文本第 2 行 base64 → 算法(2："Ed" 直签 / "ED" 先 BLAKE2b-512 预哈希) + key id(8) + 签名(64)
//   第 3 行 "trusted comment: …"，第 4 行 base64 → 对「签名(64) + 可信注释」的全局签名(64)
import { createHash, createPublicKey, verify } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";

export const PUBKEY_PLACEHOLDER = "REPLACE_WITH_UPDATER_PUBLIC_KEY";

const SPKI_ED25519_PREFIX = Buffer.from("302a300506032b6570032100", "hex");
const TRUSTED_PREFIX = "trusted comment: ";
const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;

function lines(encoded, what) {
  const text = Buffer.from(encoded.trim(), "base64").toString("utf8");
  const rows = text.split("\n").map((row) => row.replace(/\r$/, ""));
  if (!rows[0]?.startsWith("untrusted comment:")) {
    throw new Error(`${what} 不是 minisign 格式（base64 解开后第一行应为 untrusted comment）`);
  }
  return rows;
}

/** 解开 tauri.conf.json 里的公钥串；占位值或格式不对直接报错。 */
export function parsePublicKey(encoded) {
  if (!encoded || encoded === PUBKEY_PLACEHOLDER) {
    throw new Error("tauri.conf.json 的 updater 公钥还是占位值：先生成密钥对并把公钥写进去");
  }
  const raw = Buffer.from(lines(encoded, "updater 公钥")[1] ?? "", "base64");
  if (raw.length !== 42 || raw.subarray(0, 2).toString("latin1") !== "Ed") {
    throw new Error("updater 公钥长度或算法不对（应为 Ed25519 minisign 公钥）");
  }
  return { keyId: raw.subarray(2, 10), key: raw.subarray(10) };
}

/** 用公钥验 `.sig`（tauri signer 产物）对 `data` 的签名；任何一处不符都抛错并说明是哪一处。 */
export function verifySignature(data, signatureEncoded, publicKeyEncoded) {
  const pub = parsePublicKey(publicKeyEncoded);
  const rows = lines(signatureEncoded, "更新包签名");
  const sig = Buffer.from(rows[1] ?? "", "base64");
  if (sig.length !== 74) {
    throw new Error("更新包签名长度不对");
  }
  const algorithm = sig.subarray(0, 2).toString("latin1");
  if (!sig.subarray(2, 10).equals(pub.keyId)) {
    throw new Error("更新包是用另一把私钥签的：key id 与 tauri.conf.json 里的公钥不符");
  }
  const key = createPublicKey({
    key: Buffer.concat([SPKI_ED25519_PREFIX, pub.key]),
    format: "der",
    type: "spki",
  });
  const signature = sig.subarray(10);
  let message;
  if (algorithm === "ED") {
    message = createHash("blake2b512").update(data).digest();
  } else if (algorithm === "Ed") {
    message = data;
  } else {
    throw new Error(`更新包签名算法未知：${algorithm}`);
  }
  if (!verify(null, message, key, signature)) {
    throw new Error("更新包签名验不过：文件在签名之后被改动过，或公钥不对");
  }
  const trusted = rows[2] ?? "";
  if (!trusted.startsWith(TRUSTED_PREFIX)) {
    throw new Error("更新包签名缺可信注释");
  }
  const global = Buffer.from(rows[3] ?? "", "base64");
  const signed = Buffer.concat([signature, Buffer.from(trusted.slice(TRUSTED_PREFIX.length), "utf8")]);
  if (!verify(null, signed, key, global)) {
    throw new Error("更新包签名的可信注释验不过");
  }
}

/** 组装 latest.json（tauri-plugin-updater 的静态清单格式）。 */
export function buildManifest({ version, notes, pubDate, signature, url, baseUrl }) {
  if (!SEMVER.test(version)) {
    throw new Error(`版本号不是语义化版本：${version}`);
  }
  const trimmedNotes = notes.trim();
  if (trimmedNotes === "") {
    throw new Error("发布说明是空的：更新提示里会只剩一个版本号");
  }
  const expectedPrefix = `${baseUrl.replace(/\/$/, "")}/${version}/`;
  if (!url.startsWith(expectedPrefix)) {
    throw new Error(`更新包地址不在这一版的发布目录里：${url}（应以 ${expectedPrefix} 开头）`);
  }
  return {
    version,
    notes: trimmedNotes,
    pub_date: pubDate,
    platforms: {
      "darwin-aarch64": { signature: signature.trim(), url },
    },
  };
}

function args(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i += 2) {
    out[argv[i].replace(/^--/, "")] = argv[i + 1];
  }
  return out;
}

// node scripts/release-manifest.mjs --pubkey <编进这个包的公钥串> --archive <.app.tar.gz> --sig <.sig>
//   --version <v> --notes <纯文本文件> --base-url <发布根地址> --out <latest.json>
if (import.meta.url === `file://${process.argv[1]}`) {
  const a = args(process.argv.slice(2));
  const signature = readFileSync(a.sig, "utf8");
  verifySignature(readFileSync(a.archive), signature, a.pubkey);
  const name = a.archive.split("/").pop();
  const manifest = buildManifest({
    version: a.version,
    notes: readFileSync(a.notes, "utf8"),
    pubDate: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
    signature,
    url: `${a["base-url"].replace(/\/$/, "")}/${a.version}/${name}`,
    baseUrl: a["base-url"],
  });
  writeFileSync(a.out, `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(`[发布] 更新包验签通过，已写 ${a.out}`);
}
