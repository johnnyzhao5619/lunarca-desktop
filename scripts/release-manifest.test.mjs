import { test } from "node:test";
import assert from "node:assert/strict";
import { buildManifest, PUBKEY_PLACEHOLDER, verifySignature } from "./release-manifest.mjs";

// 夹具是 `tauri signer generate --ci -p ""` 现生成的一次性测试密钥（私钥已丢弃，只留公钥）与
// `tauri signer sign` 对 PAYLOAD 的真实签名——验的是 tauri 真正的产物格式，不是自己拼的。
const PAYLOAD = Buffer.from("lunarca release-manifest fixture\n");
const PUBKEY =
  "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEMzMzc3Qjg3RTdFRkNCOUIKUldTYnkrL25oM3Mzd3dHNWVPMmUwN2FXTFpBaXRtaS9QVTFqSXJjdlJUdlZuTThxdWhVb1M3cnoK";
const OTHER_PUBKEY =
  "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDEyQjAzMkVBRjc3NDU2MDIKUldRQ1ZuVDM2akt3RXA4V0RzMVdUbW5Hc3RYOEs0T0l0WW5Ga29xS2NacnJnTGJXZGMyUFlXRFkK";
const SIGNATURE =
  "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTYnkrL25oM3MzdzM5QitvSVl5Q0lhU1FoQW85aXQ2SlE0TkhTbzE1OTlQTXBMQlZEWU5LOXd1K1J4OE5wYjhOTUN0Q2lzbW1seE5UNWxxRXVzengzRGRzb0JwMzNFN3djPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkzMjMwNzgxCWZpbGU6cGF5bG9hZC50eHQKOWhHcXcvQmJxeUdVUURYajFwZjFpLzlRZGxabzJSSmY3Y3hXUzFrdjdGOG1KZU1yN3c4QWRSWmVBZjNxb3Bva0FPczdCWUZDeFpMdzNGckdYREhXQ0E9PQo=";

test("tauri signer 的真实签名能被同一对钥匙的公钥验过", () => {
  assert.doesNotThrow(() => verifySignature(PAYLOAD, SIGNATURE, PUBKEY));
});

test("内容被改过、钥匙不是一对、公钥还是占位值，一律拦下并说明原因", () => {
  assert.throws(() => verifySignature(Buffer.from("tampered\n"), SIGNATURE, PUBKEY), /验不过/);
  assert.throws(() => verifySignature(PAYLOAD, SIGNATURE, OTHER_PUBKEY), /另一把私钥/);
  assert.throws(() => verifySignature(PAYLOAD, SIGNATURE, PUBKEY_PLACEHOLDER), /占位值/);
  assert.throws(() => verifySignature(PAYLOAD, "bm90IG1pbmlzaWdu", PUBKEY), /minisign/);
});

test("清单形状：tauri 静态清单格式，地址落在这一版自己的目录", () => {
  const manifest = buildManifest({
    version: "1.2.0",
    notes: "  修好了同步卡顿\n",
    pubDate: "2026-10-28T23:00:00Z",
    signature: `${SIGNATURE}\n`,
    url: "https://lunarca.app/releases/1.2.0/Lunarca_1.2.0_aarch64.app.tar.gz",
    baseUrl: "https://lunarca.app/releases/",
  });
  assert.deepEqual(manifest, {
    version: "1.2.0",
    notes: "修好了同步卡顿",
    pub_date: "2026-10-28T23:00:00Z",
    platforms: {
      "darwin-aarch64": {
        signature: SIGNATURE,
        url: "https://lunarca.app/releases/1.2.0/Lunarca_1.2.0_aarch64.app.tar.gz",
      },
    },
  });
});

test("版本号、发布说明、地址不对时不出清单", () => {
  const base = {
    version: "1.2.0",
    notes: "说明",
    pubDate: "2026-10-28T23:00:00Z",
    signature: SIGNATURE,
    url: "https://lunarca.app/releases/1.2.0/x.tar.gz",
    baseUrl: "https://lunarca.app/releases",
  };
  assert.throws(() => buildManifest({ ...base, version: "v1.2" }), /语义化版本/);
  assert.throws(() => buildManifest({ ...base, notes: " \n" }), /发布说明是空的/);
  assert.throws(
    () => buildManifest({ ...base, url: "https://lunarca.app/releases/1.1.0/x.tar.gz" }),
    /发布目录/,
  );
});
