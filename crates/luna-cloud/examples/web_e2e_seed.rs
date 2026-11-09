//! 网页版端到端用例的种子工具（TDD-17 §8）：对着一台**开发模式**的服务端（`LUNARCA_DEV_INMEMORY=1`），
//! 以运营身份登记本地存储位置并建号，再用本 crate 的客户端（与桌面同一条路）凭激活码设口令登录、
//! 建库（托管信封，ADR-150 ①）、推若干篇笔记与附件。最后往 stdout 打一行 JSON：账户、口令与库。
//!
//! 只服务端到端用例（`apps/web/e2e/`），不随桌面应用分发。开发模式没有邮件出口，所以建号走运营
//! 「建号 → 回执里的激活码」这条路（没有邮件出口的部署本来就这样）。
//!
//! 环境变量：`LUNARCA_E2E_API`（服务端地址）、`LUNARCA_E2E_ADMIN_PASSWORD`（运营口令）、
//! `LUNARCA_E2E_BLOB_ROOT`（本地存储位置，须在服务端的 `SYNC_BLOB_STORE_ROOTS` 里）、
//! `LUNARCA_E2E_EMAIL` / `LUNARCA_E2E_PASSWORD`（要建的账户）、`LUNARCA_E2E_STATE_FILE`（这台「桌面」的
//! 凭据与库 id 落在哪个文件里，供下面两个只读子命令再用同一个设备会话；子命令同样要 `LUNARCA_E2E_API`）。
//!
//! 只读子命令（同一条桌面路径把云端的东西拉回来，给端到端核对用，不写任何东西）：
//! - `pull <库内路径>`：这个路径上的文件此刻的 `file_id`、rev 与**原样字节**（base64）——核对网页保存出的
//!   文件与编辑器写出的正文逐字节一致（PRD-21-020）；
//! - `pull-id <file_id>`：这个文件此刻的头（rev、是不是墓碑、路径）——核对改名 / 移动是同一个文件、删除留下了墓碑。
//!
//! 运营子命令（经运营接口改这个账户的状态，给「只读」用例造前提）：
//! - `fill-quota`：给这个账户指派一条同步容量上限 1 字节的配额覆盖（一小时后到期）——它已有的笔记早超过 1 字节，
//!   之后的写入回 `sync_quota_exceeded`，只含删除的提交照常放行（TDD-16 §9）。
//!
//! `cargo run -p luna-cloud --example web_e2e_seed [-- pull <路径> | pull-id <file_id>]`

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use luna_cloud::account::{AccountServer, CodeSignInDto, PasswordSignInDto, SignInTarget};
use luna_cloud::store::CredentialStore;
use luna_cloud::sync_client::{Base64Bytes, SyncCreateVault, SyncEnvelopeInput};
use luna_cloud::{CloudClient, CloudError};
use luna_sync_core::commit;
use luna_sync_core::crypto::{Cmk, KeyRing};
use luna_sync_core::manifest::{self, prepare};
use luna_sync_core::path::nfc;
use serde_json::{json, Value};

const VAULT_NAME: &str = "端到端库";
const DEVICE: &str = "E2E Seed";
const PLATFORM: &str = "macos";

/// 1×1 像素的 PNG。
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x2b, 0x5d, 0x8f, 0x0d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("缺环境变量 {name}"))
}

/// 这台「桌面」的硬件指纹、库 id 在凭据文件里的键（与客户端自己的键不撞：带 `e2e.` 前缀）。
const HARDWARE_KEY: &str = "e2e.hardware_id";
const VAULT_KEY: &str = "e2e.vault_id";
const ACCOUNT_KEY: &str = "e2e.account_id";

/// 落在一个 JSON 文件里的凭据存储：种子之后的子命令接着用同一个设备会话（刷新令牌轮换也写回这里）。每次读写都
/// 现读文件，几个实例（种子自己记库 id、客户端记令牌）不会互相盖掉。只给端到端用；文件在 `apps/web/e2e/.state/`
/// （不入库），收尾时与服务端一起清掉。
struct FileStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileStore {
    fn open(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    fn read(&self) -> BTreeMap<String, String> {
        std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn update(&self, change: impl FnOnce(&mut BTreeMap<String, String>)) {
        let _guard = self.lock.lock().expect("凭据锁");
        let mut map = self.read();
        change(&mut map);
        std::fs::write(&self.path, serde_json::to_vec(&map).expect("凭据序列化"))
            .expect("写凭据文件");
    }
}

impl CredentialStore for FileStore {
    fn get(&self, key: &str) -> Result<Option<String>, CloudError> {
        let _guard = self.lock.lock().expect("凭据锁");
        Ok(self.read().get(key).cloned())
    }

    fn set(&self, key: &str, value: &str) -> Result<(), CloudError> {
        self.update(|map| {
            map.insert(key.to_owned(), value.to_owned());
        });
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), CloudError> {
        self.update(|map| {
            map.remove(key);
        });
        Ok(())
    }
}

/// 用凭据文件里的同一个设备会话起一个客户端（与种子那次是同一台「桌面」）。
fn client_from_state() -> (CloudClient, String) {
    let store = FileStore::open(PathBuf::from(env("LUNARCA_E2E_STATE_FILE")));
    let hardware = store
        .get(HARDWARE_KEY)
        .ok()
        .flatten()
        .expect("凭据文件里没有硬件指纹：先跑种子");
    let vault = store
        .get(VAULT_KEY)
        .ok()
        .flatten()
        .expect("凭据文件里没有库 id：先跑种子");
    let client = CloudClient::new(Box::new(store), Arc::new(|| false)).with_fingerprint(
        luna_cloud::fingerprint::FingerprintProvider::with_source(Arc::new(move || {
            Some(hardware.clone())
        })),
    );
    // 种子那次登录时选的服务器（桌面把它记在账户文件里，这里直接给）。
    client.account().set_server(AccountServer {
        origin: env("LUNARCA_E2E_API"),
        display_name: None,
    });
    client.reconfigure();
    (client, vault)
}

/// 托管信封里的 CMK → 这个库的密钥环（与桌面接入已有库同一条路，ADR-150 ①）。
async fn key_ring(client: &CloudClient, vault: &str) -> KeyRing {
    let envelopes = client.sync_envelopes(vault).await.expect("取信封");
    let managed = envelopes
        .iter()
        .find(|e| e.slot == "managed")
        .expect("托管信封");
    let bytes: [u8; 32] = managed
        .wrapped_cmk
        .0
        .as_slice()
        .try_into()
        .expect("CMK 32 字节");
    let cmk = Cmk::from_bytes(u32::try_from(managed.cmk_version).expect("版本"), bytes);
    KeyRing::new(uuid::Uuid::parse_str(vault).expect("库 id"), &[cmk]).expect("密钥环")
}

/// `pull <路径>`：清单里这个路径上的文件（名字按 NFC 比）此刻的原样字节。没有这个路径 → `{"found":false}`。
async fn pull(wanted: &str) -> Value {
    let (client, vault) = client_from_state();
    let ring = key_ring(&client, &vault).await;
    let manifest = client.sync_manifest(&vault, None).await.expect("取清单");
    for item in manifest.entries.iter().filter(|e| !e.deleted) {
        let Some(ct) = &item.entry_ct else { continue };
        let file_id = uuid::Uuid::parse_str(&item.file_id).expect("file_id");
        let entry = manifest::open_entry(&ring, file_id, item.rev, &ct.0).expect("解开条目");
        if nfc(&entry.path) != nfc(wanted) {
            continue;
        }
        let mut blobs = BTreeMap::new();
        for id in &entry.chunks {
            if !blobs.contains_key(id) {
                blobs.insert(
                    id.clone(),
                    client.sync_get_blob(&vault, id).await.expect("取块"),
                );
            }
        }
        let bytes = manifest::assemble(&ring, &entry, |id| {
            blobs.get(id).cloned().ok_or_else(|| "missing".to_owned())
        })
        .expect("拼装并核对");
        return json!({
            "found": true,
            "file_id": item.file_id,
            "rev": item.rev,
            "path": entry.path,
            "bytes_b64": base64::engine::general_purpose::STANDARD.encode(bytes),
        });
    }
    json!({ "found": false })
}

/// `pull-id <file_id>`：这个文件此刻的头（rev、是不是墓碑、路径）。清单里只有活着的文件，墓碑要从变更流里看：
/// 从头读一遍，取这个文件最后一条。
async fn pull_id(wanted: &str) -> Value {
    let (client, vault) = client_from_state();
    let ring = key_ring(&client, &vault).await;
    let epoch = client
        .sync_manifest(&vault, None)
        .await
        .expect("取清单")
        .epoch;
    let mut since = 0;
    let mut last = None;
    loop {
        let page = client
            .sync_changes(&vault, &epoch, since, 500)
            .await
            .expect("取变更");
        if let Some(item) = page.entries.iter().rev().find(|e| e.file_id == wanted) {
            last = Some(item.clone());
        }
        since = page.next_seq;
        if !page.has_more {
            break;
        }
    }
    let Some(item) = last else {
        return json!({ "found": false });
    };
    let path = item.entry_ct.as_ref().map(|ct| {
        let file_id = uuid::Uuid::parse_str(&item.file_id).expect("file_id");
        manifest::open_entry(&ring, file_id, item.rev, &ct.0)
            .expect("解开条目")
            .path
    });
    json!({ "found": true, "file_id": item.file_id, "rev": item.rev, "deleted": item.deleted, "path": path })
}

/// `fill-quota`：运营建一条「同步容量 1 字节」的策略（预览 → 带预览凭据提交），再给这个账户指派一小时的覆盖。
async fn fill_quota() -> Value {
    let api = env("LUNARCA_E2E_API");
    let http = reqwest::Client::new();
    let session = admin(&http, &api, &env("LUNARCA_E2E_ADMIN_PASSWORD")).await;
    let store = FileStore::open(PathBuf::from(env("LUNARCA_E2E_STATE_FILE")));
    let account = store
        .get(ACCOUNT_KEY)
        .ok()
        .flatten()
        .expect("凭据文件里没有账户 id：先跑种子");
    let name = format!("e2e 容量已满 {}", uuid::Uuid::new_v4());
    let limits: Vec<Value> = [
        "tokens_in",
        "tokens_out",
        "asr_seconds",
        "storage_byte_hours",
        "egress_bytes",
        "requests",
        "sync_storage_bytes",
    ]
    .iter()
    .map(|dimension| {
        let limit = (*dimension == "sync_storage_bytes").then_some(1);
        json!({ "dimension": dimension, "limit_value": limit })
    })
    .collect();
    let policy = json!({
        "name": name,
        "period": "monthly",
        "limits": limits,
        "allowed_capabilities": null,
        "sync_max_file_bytes": 2_147_483_648_i64,
        "max_devices": null,
    });
    let mut change = policy.clone();
    change["kind"] = json!("policy_upsert");
    let preview = admin_post(
        &http,
        format!("{api}/v1/admin/quota/preview"),
        &session,
        change,
    )
    .await;
    let mut upsert = policy;
    upsert["reason"] = json!("e2e：只读状态");
    upsert["preview_token"] = preview["preview_token"].clone();
    admin_send(
        &http,
        format!("{api}/v1/admin/quota/policies"),
        &session,
        upsert,
    )
    .await;

    let listed: Value = http
        .get(format!("{api}/v1/admin/quota/policies"))
        .query(&[("q", name.as_str())])
        .header(reqwest::header::COOKIE, &session.0)
        .send()
        .await
        .expect("列策略")
        .json()
        .await
        .expect("策略应答");
    let policy_id = listed["items"]
        .as_array()
        .and_then(|items| items.iter().find(|p| p["name"] == name.as_str()))
        .and_then(|p| p["id"].as_str())
        .expect("新建的策略")
        .to_owned();
    let expires_at = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let preview = admin_post(
        &http,
        format!("{api}/v1/admin/quota/preview"),
        &session,
        json!({ "kind": "account_override", "account_ids": [account], "policy_id": policy_id, "expires_at": expires_at }),
    )
    .await;
    admin_send(
        &http,
        format!("{api}/v1/admin/quota/assign"),
        &session,
        json!({
            "account_ids": [account],
            "policy_id": policy_id,
            "reason": "e2e：只读状态",
            "expires_at": expires_at,
            "preview_token": preview["preview_token"],
        }),
    )
    .await;
    json!({ "account_id": account, "policy_id": policy_id })
}

/// 推上去的文件：笔记（含 frontmatter、双链、图片、Mermaid、数学）、XSS 夹具、附件、录音与一个隐藏文件。
fn files() -> Vec<(String, Vec<u8>)> {
    let long_line = "长".repeat(12_000);
    let notes = [
        (
            "欢迎.md",
            "---\ntitle: 欢迎\ntags: [入门, 网页]\n---\n# 欢迎使用 Lunarca\n\n这是同步到云端的第一篇笔记。\n\n\
             下一步看 [[计划]]，或者打开 [[项目/图表|图表那篇]]。\n\n![封面](附件/封面.png)\n\n\
             外链：[Lunarca](https://lunarca.app) 与 [写信](mailto:hello@example.com)。\n",
        ),
        (
            "项目/计划.md",
            "# 计划\n\n- [x] 建库\n- [ ] 网页版\n\n| 列一 | 列二 |\n| --- | --- |\n| 甲 | 乙 |\n\n\
             质能方程 $E=mc^2$。\n\n$$\n\\int_0^1 x\\,dx = \\tfrac{1}{2}\n$$\n\n```rust\nfn main() {}\n```\n\n\
             回到 [欢迎](../欢迎.md)。\n",
        ),
        (
            "项目/图表.md",
            "# 图表\n\n```mermaid\ngraph TD\n  A[开始] --> B[结束]\n```\n",
        ),
        ("周报 Week 12.md", "# 周报\n\n本周完成了网页版骨架。\n"),
    ];
    let xss = format!(
        "# XSS 夹具\n\n<script>window.__xss = 'script'</script>\n\n<img src=x onerror=\"window.__xss = 'img'\">\n\n\
         [点我](javascript:window.__xss='link')\n\n<svg onload=\"window.__xss = 'svg'\"><circle r=\"5\"/></svg>\n\n\
         ![恶意图](../附件/恶意.svg)\n\n{long_line}\n"
    );
    let mut out: Vec<(String, Vec<u8>)> = notes
        .iter()
        .map(|(path, text)| ((*path).to_owned(), text.as_bytes().to_vec()))
        .collect();
    out.push(("安全/XSS 夹具.md".into(), xss.into_bytes()));
    out.push(("附件/封面.png".into(), PNG.to_vec()));
    out.push((
        "附件/恶意.svg".into(),
        b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\"><script>parent.__xss='svg-file'</script><rect width=\"8\" height=\"8\"/></svg>".to_vec(),
    ));
    out.push(("录音/会议.m4a".into(), vec![0u8; 4096]));
    // 网页编辑的用例各用一篇（同一个服务端上的用例依次跑，互不干扰）；文件夹名排在已有几个之后。
    for name in [
        "保存", "合并", "冲突", "离线", "吊销", "改名", "移动", "删除", "封存",
    ] {
        out.push((
            format!("写作/{name}.md"),
            format!("# {name}\n\n第一段。\n\n第二段。\n\n第三段。\n").into_bytes(),
        ));
    }
    // 格式工具条与属性面板的用例（PRD-21-020）：带 frontmatter，改出来的字节与桌面同一套规则。
    out.push((
        "写作/格式.md".into(),
        "---\nstatus: 草稿\ndone: false\n---\n# 格式\n\n第一段。\n\n第二段。\n\n第三段。\n"
            .as_bytes()
            .to_vec(),
    ));
    out.push((".luna/bookmarks.json".into(), b"[]".to_vec()));
    out.extend(data_files());
    out
}

/// 网页数据功能（附件下载与上传、最近删除、版本历史、全文搜索与标签）的用例用的文件：一份 PDF 附件、各用一篇
/// 的笔记、一篇带中文与韩文标签和正文词的笔记，以及库级设置（附件放到指定的「附件」文件夹）。
fn data_files() -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = ["上传", "历史", "回收"]
        .iter()
        .map(|name| {
            (
                format!("写作/{name}.md"),
                format!("# {name}\n\n第一段。\n\n第二段。\n\n第三段。\n").into_bytes(),
            )
        })
        .collect();
    out.push((
        "项目/韩文.md".into(),
        "---\ntags: [多语言]\n---\n# 韩文\n\n안녕하세요, 오늘 회의 #한국어 #项目/网页\n\n全文搜索要找的词：琥珀色的月亮。\n"
            .as_bytes()
            .to_vec(),
    ));
    out.push((
        "附件/说明.pdf".into(),
        b"%PDF-1.4\n% e2e\n1 0 obj << /Type /Catalog >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n"
            .to_vec(),
    ));
    // 扩展名认不出来、内容也不是文本：网页要打开成能下载的附件页。
    out.push((
        "附件/数据.bin".into(),
        vec![0, 159, 146, 150, 255, 254, 0, 1],
    ));
    out.push((
        ".luna/settings.toml".into(),
        b"schema_version = 1\n\n[values]\n\"vault.attachment_location\" = \"specified_folder\"\n\"vault.attachment_folder\" = \"\xe9\x99\x84\xe4\xbb\xb6\"\n"
            .to_vec(),
    ));
    out
}

async fn admin(http: &reqwest::Client, api: &str, password: &str) -> (String, String) {
    let login = http
        .post(format!("{api}/v1/admin/login"))
        .json(&json!({ "username": "admin", "password": password }))
        .send()
        .await
        .expect("运营登录");
    assert!(login.status().is_success(), "运营登录：{}", login.status());
    let cookie = login
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .expect("运营会话 Cookie")
        .to_owned();
    let body: Value = login.json().await.expect("运营登录应答");
    (
        cookie,
        body["csrf_token"].as_str().expect("csrf").to_owned(),
    )
}

/// 只要成功、不要应答体的运营提交（有些接口回 204）。
async fn admin_send(http: &reqwest::Client, url: String, session: &(String, String), body: Value) {
    let response = http
        .post(&url)
        .header(reqwest::header::COOKIE, &session.0)
        .header("x-csrf-token", &session.1)
        .header("idempotency-key", uuid::Uuid::new_v4().to_string())
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|_| panic!("{url}"));
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    assert!(status.is_success(), "{url} → {status}：{text}");
}

async fn admin_post(
    http: &reqwest::Client,
    url: String,
    session: &(String, String),
    body: Value,
) -> Value {
    let response = http
        .post(&url)
        .header(reqwest::header::COOKIE, &session.0)
        .header("x-csrf-token", &session.1)
        .header("idempotency-key", uuid::Uuid::new_v4().to_string())
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|_| panic!("{url}"));
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    assert!(status.is_success(), "{url} → {status}：{body}");
    body
}

/// 默认存储位置：登记 → 自检 → 启用 → 设为默认（ADR-149）。已有默认位置就不动。
async fn ensure_location(http: &reqwest::Client, api: &str, session: &(String, String)) {
    let listed: Value = http
        .get(format!("{api}/sync/v1/admin/storage-locations"))
        .header(reqwest::header::COOKIE, &session.0)
        .send()
        .await
        .expect("列存储位置")
        .json()
        .await
        .expect("存储位置应答");
    let has_default = listed
        .as_array()
        .or_else(|| listed["locations"].as_array())
        .is_some_and(|all| all.iter().any(|l| l["is_default"] == true));
    if has_default {
        return;
    }
    let base = format!("{api}/sync/v1/admin/storage-locations");
    let created = admin_post(
        http,
        base.clone(),
        session,
        json!({
            "display_name": "e2e",
            "config": { "kind": "local_fs", "root": env("LUNARCA_E2E_BLOB_ROOT") },
            "capacity_soft_bytes": 0,
            "capacity_hard_bytes": 0,
            "secret": null,
            "reason": null,
        }),
    )
    .await;
    let id = created["id"].as_str().expect("位置 id").to_owned();
    admin_post(http, format!("{base}/{id}/self-check"), session, json!({})).await;
    admin_post(
        http,
        format!("{base}/{id}/activate"),
        session,
        json!({ "reason": "" }),
    )
    .await;
    admin_post(
        http,
        format!("{base}/{id}/default"),
        session,
        json!({ "reason": "e2e" }),
    )
    .await;
}

fn store_account(id: &str) {
    FileStore::open(PathBuf::from(env("LUNARCA_E2E_STATE_FILE")))
        .set(ACCOUNT_KEY, id)
        .expect("记下账户 id");
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => seed().await,
        [cmd, path] if cmd == "pull" => println!("{}", pull(path).await),
        [cmd, id] if cmd == "pull-id" => println!("{}", pull_id(id).await),
        [cmd] if cmd == "fill-quota" => println!("{}", fill_quota().await),
        _ => panic!("用法：web_e2e_seed [pull <路径> | pull-id <file_id> | fill-quota]"),
    }
}

async fn seed() {
    let api = env("LUNARCA_E2E_API");
    let email = env("LUNARCA_E2E_EMAIL");
    let password = env("LUNARCA_E2E_PASSWORD");
    let http = reqwest::Client::new();

    let session = admin(&http, &api, &env("LUNARCA_E2E_ADMIN_PASSWORD")).await;
    ensure_location(&http, &api, &session).await;
    let receipt = admin_post(
        &http,
        format!("{api}/v1/admin/accounts"),
        &session,
        json!({ "email": email }),
    )
    .await;
    store_account(receipt["account_id"].as_str().expect("回执里的账户 id"));
    let code = receipt["delivery"]["activation_code"]["code"]
        .as_str()
        .expect("回执里的激活码（开发模式没有邮件出口）")
        .to_owned();

    // 与桌面同一条登录路：邮箱 + 服务器 → 激活码 → 设口令。
    let hardware = uuid::Uuid::new_v4().to_string();
    let store = FileStore::open(PathBuf::from(env("LUNARCA_E2E_STATE_FILE")));
    store.set(HARDWARE_KEY, &hardware).expect("记下硬件指纹");
    let client = CloudClient::new(Box::new(store), Arc::new(|| false)).with_fingerprint(
        luna_cloud::fingerprint::FingerprintProvider::with_source(Arc::new(move || {
            Some(hardware.clone())
        })),
    );
    client
        .sign_in_begin(SignInTarget::Server {
            email: email.clone(),
            input: api.clone(),
        })
        .await
        .expect("登录第一步");
    let step = client
        .sign_in_code(Some(&code), DEVICE, PLATFORM)
        .await
        .expect("激活码登录");
    assert!(matches!(step, CodeSignInDto::SetPassword), "{step:?}");
    let signed = client
        .sign_in_set_password(&password, DEVICE, PLATFORM)
        .await
        .expect("设口令");
    assert!(matches!(signed, PasswordSignInDto::SignedIn), "{signed:?}");

    // 只建账户不建库：网页第一次进入的用例（ADR-178 ③）。
    if std::env::var_os("LUNARCA_E2E_NO_VAULT").is_some() {
        println!("{{}}");
        return;
    }

    // 建库：CMK 在客户端生成，作为托管信封交给服务端（ADR-150 ①）。
    let vault_id = uuid::Uuid::new_v4();
    let cmk = Cmk::generate(1);
    let ring = KeyRing::new(vault_id, std::slice::from_ref(&cmk)).expect("密钥环");
    let keys = ring.current();
    let vault = client
        .sync_create_vault(&SyncCreateVault {
            id: vault_id.to_string(),
            name_ct: Base64Bytes(keys.seal_vault_name(VAULT_NAME)),
            envelope: SyncEnvelopeInput {
                slot: "managed".into(),
                cmk_version: 1,
                wrapped_cmk: Base64Bytes(cmk.expose_bytes().to_vec()),
                kdf_params: json!({}),
            },
        })
        .await
        .expect("建库");
    FileStore::open(PathBuf::from(env("LUNARCA_E2E_STATE_FILE")))
        .set(VAULT_KEY, &vault.id)
        .expect("记下库 id");

    let mut items = Vec::new();
    let mut paths = Vec::new();
    for (index, (path, bytes)) in files().into_iter().enumerate() {
        let mtime = 1_780_000_000_000 + i64::try_from(index).unwrap() * 60_000;
        let prepared = prepare(keys, &path, &bytes, Some(mtime));
        let ids: Vec<String> = prepared.chunks.iter().map(|c| c.id.clone()).collect();
        let missing = client
            .sync_missing_blobs(&vault.id, &ids)
            .await
            .expect("问缺块");
        for chunk in prepared.chunks.iter().filter(|c| missing.contains(&c.id)) {
            client
                .sync_put_blob(&vault.id, &chunk.id, &chunk.sealed, &hex(&chunk.ct_sha256))
                .await
                .expect("上传块");
        }
        let file_id = uuid::Uuid::now_v7();
        items.push(commit::put(keys, file_id, None, &prepared));
        paths.push(json!({ "path": path, "file_id": file_id.to_string() }));
    }
    client
        .sync_commit(&vault.id, &uuid::Uuid::new_v4().to_string(), &items)
        .await
        .expect("提交");

    println!(
        "{}",
        json!({
            "email": email,
            "password": password,
            "vault_id": vault.id,
            "vault_name": VAULT_NAME,
            "files": paths,
        })
    );
}
