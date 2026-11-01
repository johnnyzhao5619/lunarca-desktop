//! 信任锚：组织配置文件 + 两把签名公钥的**钉住**状态。
//!
//! 协议裁决本身是 ADR-060（主路径 = 组织配置
//! 文件、副路径 = TOFU 钉两把**签名公钥**而非 TLS 证书指纹、配置文件刻意不签名），本模块
//! 只落地它，理由不在此复述；落地期的裁决见 ADR-071。
//!
//! # 钉住值就是生效的验签公钥
//!
//! 本模块记的是**「这台服务端的身份是这两把钥」**这条断言，以及它是怎么来的（导入 / 口令登录 /
//! 用户核对 / 轮换）。[`crate::config::CloudConfig`] 的两把验签公钥直接取自它（调试构建的
//! env 覆盖除外），没有第二份可以分叉。确认之前钉住值一个字节都不变——这正是 ADR-060 ②
//! 「变更必须阻断并要求重新确认」得以成立的原因。自动采用服务端新报的钥会让防回滚水位
//! 当场被架空（换一把钥重签一份倒退的目录即可）。
//!
//! # 分桶
//!
//! 钉住状态按**既有服务端分桶**（[`crate::config::server_bucket`]）持久化，与钥匙链凭据
//! （`client::CloudClient::scoped`）和目录防回滚水位（`catalog::Target::bucket`）同一判据。
//! 换服务器 = 换桶 = 重新建立信任。不发明第二套分桶：三处判断一旦分叉，用户会遇到
//! 「会话还在、目录从头开始、信任锚却说没变」这种没人解释得清的半状态。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// 组织配置文件的 `kind` 判别字段（逐字契约，与控制台导出端对齐）。
pub const ORG_CONFIG_KIND: &str = "lunarca-org-config";

/// 本客户端支持的组织配置文件格式版本。
///
/// **不等于它就如实拒绝**，并提示升级客户端。这不是兼容层缺失，是
/// ADR-046 的诚实拒绝语义：猜一个更新版本的
/// 文件里「多出来的字段大概不要紧」，等于用一份读不全的信任锚去建立信任。
pub const ORG_CONFIG_FORMAT_VERSION: u32 = 1;

/// 公钥分发端点（公开只读、无鉴权——公钥非秘密，服务端启动日志本就打印它）。
pub const TRUST_KEYS_PATH: &str = "/v1/trust/keys";

/// `GET /v1/trust/keys` 的响应格式版本。
pub const TRUST_KEYS_FORMAT_VERSION: u32 = 1;

/// 应用数据目录下的钉住状态文件名。
///
/// 与目录状态（`cloud-catalog.json`）分文件：两者的生命周期完全不同——目录状态是
/// **可重建的缓存**（解析失败当作没有，下次刷新即恢复），而钉住状态是**用户做过的一次
/// 安全决定**。混在一个文件里，一次目录缓存损坏就会顺手把信任锚也当成可丢弃的东西。
pub const TRUST_STATE_FILE: &str = "cloud-trust.json";

/// 钉住是怎么来的。
///
/// 只作**如实展示**用（用户该知道自己当初是导入了一份文件、还是首连时确认过一次指纹），
/// **不参与判定**：两条路径钉住之后的语义完全相同。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TrustProvenance {
    /// 主路径：导入组织配置文件（ADR-060 ①）。
    Imported,
    /// 口令登录（ADR-158 ③）：两钥随信任包送达，信任包用 OPAQUE 会话密钥解开——对端拿不到
    /// 这个账户的注册记录就造不出来。自签部署的新设备走的就是这一条。
    Password,
    /// 用户核对后确认（ADR-060 ②）：经可信 TLS（公网证书）首次取到、或之后观察到变更时，
    /// 用户逐字核对两组公钥后钉住。
    Tofu,
    /// 轮换链（ADR-124）：服务端换了钥，而那次换钥由**上一代私钥**签名背书、客户端沿链
    /// 验过，于是自动重钉。它同时是界面上那句「这一组钥是怎么来的」——用户会被登出一次，
    /// 而这一行是他唯一能看到的解释。
    Rotated,
}

impl TrustProvenance {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Imported => "imported",
            Self::Password => "password",
            Self::Tofu => "tofu",
            Self::Rotated => "rotated",
        }
    }
}

/// 钉在某个服务端桶上的信任锚。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinnedAnchor {
    /// 64 位小写 hex。
    pub lease_pubkey: String,
    /// 目录**根**公钥（64 位小写 hex，ADR-169 ⑥）：只验根签的在线目录密钥清单，日常换在线目录钥不动它。
    pub catalog_pubkey: String,
    /// 钉住时刻（Unix 秒）。
    pub pinned_at: i64,
    pub provenance: TrustProvenance,
    /// **信息性字段，绝不参与判定**（修 7）。部署 id 可以合法变更（重装、改名），
    /// 把它算进判据只会制造假阳性的「信任锚变了」；**钥才是身份**。
    /// 确认对话框展示它，是为了让用户在核对时多一个人类可读的坐标。
    pub deployment_id: String,
}

/// 端点自报、且与钉住值**不一致**（或尚未钉住）的一组钥。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferedAnchor {
    pub lease_pubkey: String,
    pub catalog_pubkey: String,
    pub deployment_id: String,
}

impl OfferedAnchor {
    /// 与已钉住的两钥逐字相等？（只比钥，不比 `deployment_id`——见 [`PinnedAnchor`]。）
    #[must_use]
    pub fn matches(&self, pinned: &PinnedAnchor) -> bool {
        self.lease_pubkey == pinned.lease_pubkey && self.catalog_pubkey == pinned.catalog_pubkey
    }
}

/// 一次比对的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustVerdict {
    /// env 旁路在场 —— 钉住比对**整体跳过**（修 6）。
    ///
    /// 调试构建的 env 覆盖优先于钉住值（[`crate::config::CloudConfig::derive`]），混着比对
    /// 会出现「钉住说钥变了、而实际生效的是 env 那把」这种自相矛盾的界面。发行构建不认 env，
    /// 永远走不到这一支。
    Bypassed,
    /// 该桶尚未钉住 —— 首连，等用户确认一次（TOFU）。
    Unpinned,
    /// 与钉住一致。
    Match,
    /// 与钉住不一致 —— **阻断**，等用户显式重新确认。
    Changed,
}

/// 钉住状态的持有者（按服务端桶）。
///
/// # 为什么「冲突观察」不持久化
///
/// [`Self::offered`] 只活在内存里：阻断态必须由**再和服务端说一次话**重新得出。持久化它
/// 的话，一次网络抖动期间的误判会变成一张重启也不消失、而用户无法验证的红色横幅；
/// 而真正的变更下一次通信立刻会再被发现。钉住值本身则必须跨重启存活——那是用户做过的
/// 一次安全决定。
#[derive(Default)]
pub struct TrustStore {
    path: Mutex<Option<PathBuf>>,
    pinned: Mutex<BTreeMap<String, PinnedAnchor>>,
    offered: Mutex<BTreeMap<String, OfferedAnchor>>,
    /// 上一次取公钥的失败原因码（同样只活在内存里，理由同 `offered`）。
    ///
    /// 有它才能如实说「这一次没能核对成信任锚，因为端点上没有这条路 / 连不上」。
    /// 没有它的话，一台根本没有 `/v1/trust/keys` 的服务端会让界面永远停在「未钉住」，
    /// 而用户点多少次「测试连接」都不会知道为什么。
    last_error: Mutex<BTreeMap<String, String>>,
    /// 冲突来自一次**导入**时，记下那份文件里的地址。
    ///
    /// 修 3 的落地形状：导入撞上既有钉住时**不直接覆盖**，而是把它记成一次「待确认的
    /// 变更」，走与 TOFU 变更**完全同一个**确认对话框与确认命令。多开一条"导入专用的
    /// 覆盖路径"就是多一个绕过变更阻断的入口，而它长得像一个正常功能。
    pending_import: Mutex<BTreeMap<String, String>>,
    /// 冲突来自一次**口令登录**的信任包（ADR-158 ③）：确认后钉住的来源记为 `Password`。
    /// 与 `pending_import` 同一套确认流程，只差来源。
    pending_password: Mutex<std::collections::BTreeSet<String>>,
}

impl TrustStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 注入持久化文件路径并装载既有钉住（插件 setup 调用；测试直接调）。
    pub fn attach_path(&self, path: PathBuf) {
        let loaded = read_state_file(&path).unwrap_or_default();
        *self.pinned.lock().expect("trust pinned mutex poisoned") = loaded;
        *self.path.lock().expect("trust path mutex poisoned") = Some(path);
    }

    /// 该桶已钉住的信任锚。
    #[must_use]
    pub fn pinned(&self, bucket: &str) -> Option<PinnedAnchor> {
        self.pinned
            .lock()
            .expect("trust pinned mutex poisoned")
            .get(bucket)
            .cloned()
    }

    /// 该桶当前被观察到的冲突（或首连待确认）的一组钥。
    #[must_use]
    pub fn offered(&self, bucket: &str) -> Option<OfferedAnchor> {
        self.offered
            .lock()
            .expect("trust offered mutex poisoned")
            .get(bucket)
            .cloned()
    }

    /// 把端点自报的一组钥与钉住值比对，并按结论记下/清除「待确认」。
    ///
    /// **本方法绝不改写钉住值**。自动采用新钥 = 防回滚水位被架空（ADR-060 ②）：
    /// 拿得到域名的攻击者换一把自己的钥，就能重签一份任意旧版本的目录。钉住只能由
    /// 用户的显式确认（[`Self::pin`]）改变。
    pub fn record_offer(&self, bucket: &str, offered: OfferedAnchor) -> TrustVerdict {
        if env_override_present() {
            self.clear_offer(bucket);
            return TrustVerdict::Bypassed;
        }
        let verdict = match self.pinned(bucket) {
            None => TrustVerdict::Unpinned,
            Some(pinned) if offered.matches(&pinned) => TrustVerdict::Match,
            Some(_) => TrustVerdict::Changed,
        };
        match verdict {
            TrustVerdict::Match | TrustVerdict::Bypassed => self.clear_offer(bucket),
            TrustVerdict::Unpinned | TrustVerdict::Changed => {
                self.offered
                    .lock()
                    .expect("trust offered mutex poisoned")
                    .insert(bucket.to_owned(), offered);
            }
        }
        verdict
    }

    /// 不联网地重新评估该桶（供零网络的状态查询用）。
    #[must_use]
    pub fn verdict(&self, bucket: &str) -> TrustVerdict {
        if env_override_present() {
            return TrustVerdict::Bypassed;
        }
        let Some(pinned) = self.pinned(bucket) else {
            return TrustVerdict::Unpinned;
        };
        match self.offered(bucket) {
            Some(offered) if !offered.matches(&pinned) => TrustVerdict::Changed,
            _ => TrustVerdict::Match,
        }
    }

    /// 钉住（首次钉 / 用户确认换钥后重钉）。**唯一**改写钉住值的入口。
    ///
    /// 三重重置（水位 + 缓存 + 凭据，修 2）**不在这里**：它涉及目录客户端与钥匙链，
    /// 两者都不归本模块。收口在装配侧的重钉命令里一次做完——把它拆进这里会得到
    /// 「钉住换了、而水位/凭据没换」的半个重钉，且没有任何东西会因此变红。
    pub fn pin(&self, bucket: &str, anchor: PinnedAnchor) {
        {
            let mut pinned = self.pinned.lock().expect("trust pinned mutex poisoned");
            pinned.insert(bucket.to_owned(), anchor);
        }
        self.clear_offer(bucket);
        self.pending_import
            .lock()
            .expect("trust pending mutex poisoned")
            .remove(bucket);
        self.pending_password
            .lock()
            .expect("trust pending mutex poisoned")
            .remove(bucket);
        self.persist();
    }

    /// 口令登录送达的两钥撞上了该桶的既有钉住：记成待确认的变更（与导入同一个确认流程），
    /// 绝不直接覆盖——口令认证了「这是那台部署」，但「它换了签名钥」仍要用户知情（ADR-060 ②）。
    pub fn record_password_conflict(&self, bucket: &str, offered: OfferedAnchor) -> TrustVerdict {
        let verdict = self.record_offer(bucket, offered);
        let mut pending = self
            .pending_password
            .lock()
            .expect("trust pending mutex poisoned");
        if verdict == TrustVerdict::Changed {
            pending.insert(bucket.to_owned());
        } else {
            pending.remove(bucket);
        }
        verdict
    }

    /// 待确认的变更是不是来自一次口令登录。
    #[must_use]
    pub fn pending_password(&self, bucket: &str) -> bool {
        self.pending_password
            .lock()
            .expect("trust pending mutex poisoned")
            .contains(bucket)
    }

    /// 一次**导入**撞上了该桶的既有钉住（修 3）：记成待确认的变更，绝不直接覆盖。
    ///
    /// **同一时刻至多一份待确认的导入**：界面只presents得了一个确认对话框，故新的导入
    /// 顶掉上一份未确认的（连同它那条已被顶掉的观察）。不维持这条不变量的话，
    /// [`Self::pending_bucket`] 就得在多份待确认之间猜一个——而猜错的后果是用户核对着
    /// A 的指纹、确认下去的却是 B。
    pub fn record_import_conflict(
        &self,
        bucket: &str,
        offered: OfferedAnchor,
        server_url: &str,
    ) -> TrustVerdict {
        let verdict = self.record_offer(bucket, offered);
        if verdict == TrustVerdict::Changed {
            let superseded: Vec<String> = {
                let mut pending = self
                    .pending_import
                    .lock()
                    .expect("trust pending mutex poisoned");
                let superseded = pending.keys().filter(|b| *b != bucket).cloned().collect();
                pending.clear();
                pending.insert(bucket.to_owned(), server_url.to_owned());
                superseded
            };
            // 被顶掉的那些观察一并清掉：它们已经不是「此刻要用户处理的那一件」，而留着
            // 会让 `pending_bucket` 面对多个候选。真正的冲突下一次通信立刻会被再发现。
            for stale in superseded {
                self.clear_offer(&stale);
            }
        }
        verdict
    }

    /// 此刻**需要用户确认**的是哪一桶（`None` = 没有待确认的观察）。
    ///
    /// **不能想当然地用「当前配置的那一桶」**：一次导入的冲突记在**文件里那台**的桶上
    /// （[`Self::record_import_conflict`]），而它未必等于当前 `api_base` 的桶——用户完全
    /// 可能连着 A、导入的是 B 的组织配置文件（B 之前钉过、这次换了钥）。按当前桶去找
    /// 待确认项会一无所获，于是界面显示「需要确认」而确认命令回「没有待确认的信任锚」，
    /// 用户被卡在一个自相矛盾的死胡同里（fail-closed，但那条路走不通）。
    ///
    /// 优先当前桶：它是用户正看着的那一台；只有它没有待确认时，才轮到那份导入。
    #[must_use]
    pub fn pending_bucket(&self, current: &str) -> Option<String> {
        if self.offered(current).is_some() {
            return Some(current.to_owned());
        }
        let pending = self
            .pending_import
            .lock()
            .expect("trust pending mutex poisoned");
        // 不变量：至多一条（见 `record_import_conflict`）。仍要求它确有观察在手——
        // 没有观察就没有可确认的那一组钥。
        pending
            .keys()
            .find(|bucket| self.offered(bucket).is_some())
            .cloned()
    }

    /// 待确认的变更是不是来自一次导入；`Some` 里是那份文件写的地址。
    #[must_use]
    pub fn pending_import(&self, bucket: &str) -> Option<String> {
        self.pending_import
            .lock()
            .expect("trust pending mutex poisoned")
            .get(bucket)
            .cloned()
    }

    fn clear_offer(&self, bucket: &str) {
        self.offered
            .lock()
            .expect("trust offered mutex poisoned")
            .remove(bucket);
    }

    /// 记下这一次取公钥的失败原因（`None` = 这一次成功了，清除旧原因）。
    pub fn record_error(&self, bucket: &str, reason: Option<&str>) {
        let mut errors = self.last_error.lock().expect("trust error mutex poisoned");
        match reason {
            Some(reason) => {
                errors.insert(bucket.to_owned(), reason.to_owned());
            }
            None => {
                errors.remove(bucket);
            }
        }
    }

    #[must_use]
    pub fn last_error(&self, bucket: &str) -> Option<String> {
        self.last_error
            .lock()
            .expect("trust error mutex poisoned")
            .get(bucket)
            .cloned()
    }

    /// 写状态文件。
    ///
    /// 与目录状态**不同**：写失败不静默降级。钉住写不下去意味着这次安全确认活不过
    /// 重启，而用户会以为自己已经确认过了——下一次启动又被同一张横幅拦住，且他无从
    /// 知道为什么。故留一条 `eprintln`（零 PII：只有桶名与错误串）。
    fn persist(&self) {
        let path = self.path.lock().expect("trust path mutex poisoned").clone();
        let Some(path) = path else { return };
        let snapshot = self
            .pinned
            .lock()
            .expect("trust pinned mutex poisoned")
            .clone();
        let Ok(bytes) = serde_json::to_vec(&snapshot) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(err) = std::fs::write(&path, bytes) {
            eprintln!("[luna-cloud] 信任锚写盘失败，本次确认活不过重启：{err}");
        }
    }
}

// ---- 展示 DTO（tauri-specta 导出为 TS；前端只渲染，判定全在这一侧）----

/// 信任锚的四种状态。**判定在 Rust**（ADR-003）：前端拿到哪一个就渲染哪一个，
/// 不得自己拿 `pinned`/`offered` 两个字段再推一遍——那是第二份会漂的判据。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TrustStateDto {
    /// env 公钥旁路在场，钉住整体不参与判定（修 6；dev 排障档）。
    Bypassed,
    /// 尚未钉住。`offered` 非空时 = 已取回两钥、等用户确认一次（TOFU）。
    Unpinned,
    /// 已钉住且一致。
    Pinned,
    /// 已钉住、但端点自报的不一致 —— 阻断态。
    Changed,
    /// 内置官方后端：身份由公共证书与随应用编入的公钥担保，不走钉住（登录流程对它跳过
    /// 核对），故不能说「还没有核验」——那句话对它永远不会变成真的。
    Official,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct AnchorKeysDto {
    /// 完整 64 位 hex 按 4 字符分组（[`group_hex`]；**不是**短指纹）。
    pub lease_pubkey: String,
    pub catalog_pubkey: String,
    /// 信息性参考，不参与判定（见 [`PinnedAnchor::deployment_id`]）。
    pub deployment_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct PinnedAnchorDto {
    #[serde(flatten)]
    pub keys: AnchorKeysDto,
    /// Unix 秒（u32：specta 禁 64 位整数入 TS，与 `lease_expires_at` 同口径）。
    pub pinned_at: u32,
    pub provenance: TrustProvenance,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct TrustStatusDto {
    /// 这是哪一台（服务端桶名，与凭据/目录水位同一判据）。
    pub server: String,
    pub state: TrustStateDto,
    /// 已钉住的那一组（`None` = 从未钉过）。
    pub pinned: Option<PinnedAnchorDto>,
    /// 端点自报、待确认或冲突的那一组（`None` = 这一刻没有待处理的观察）。
    pub offered: Option<AnchorKeysDto>,
    /// 上次取公钥的失败原因码（`network` / `server_error` / `unsupported_format` / …）；
    /// `None` = 上次成功，或还没取过。
    pub last_error: Option<String>,
}

impl TrustStore {
    /// 某个桶的完整状态快照（**零网络**）。
    #[must_use]
    pub fn status(&self, bucket: &str) -> TrustStatusDto {
        let pinned = self.pinned(bucket);
        let offered = self.offered(bucket);
        TrustStatusDto {
            server: bucket.to_owned(),
            state: match self.verdict(bucket) {
                TrustVerdict::Bypassed => TrustStateDto::Bypassed,
                TrustVerdict::Unpinned => TrustStateDto::Unpinned,
                TrustVerdict::Match => TrustStateDto::Pinned,
                TrustVerdict::Changed => TrustStateDto::Changed,
            },
            pinned: pinned.map(|anchor| PinnedAnchorDto {
                keys: AnchorKeysDto {
                    lease_pubkey: group_hex(&anchor.lease_pubkey),
                    catalog_pubkey: group_hex(&anchor.catalog_pubkey),
                    deployment_id: anchor.deployment_id,
                },
                pinned_at: u32::try_from(anchor.pinned_at).unwrap_or(u32::MAX),
                provenance: anchor.provenance,
            }),
            offered: offered.map(|anchor| AnchorKeysDto {
                lease_pubkey: group_hex(&anchor.lease_pubkey),
                catalog_pubkey: group_hex(&anchor.catalog_pubkey),
                deployment_id: anchor.deployment_id,
            }),
            last_error: self.last_error(bucket),
        }
    }
}

fn read_state_file(path: &PathBuf) -> Option<BTreeMap<String, PinnedAnchor>> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// 调试构建的 env 公钥旁路是否在场（修 6）；发行构建恒为 `false`。
#[must_use]
pub fn env_override_present() -> bool {
    [
        crate::config::ENV_LEASE_PUBKEY,
        crate::config::ENV_CATALOG_PUBKEY,
    ]
    .iter()
    .any(|key| crate::config::debug_env(key).is_some())
}

// ---- 展示：完整 hex 分组 ----

/// 每组的字符数（64 hex → 16 组）。
const HEX_GROUP: usize = 4;

/// 把 64 位 hex 切成 4 字符一组，便于与服务端启动日志**逐字**比对。
///
/// **刻意不发明短指纹**（不截断、不做 SHA、不搞 base32 单词表）。服务端启动日志打印的
/// 就是完整 hex，自造一种短格式会让两边根本对不上号——而用户要做的恰恰是「把屏幕上这串
/// 和 SSH 上那串比一遍」。分组只加空白、不改任何一个字符。
#[must_use]
pub fn group_hex(hex: &str) -> String {
    let mut out = String::with_capacity(hex.len() + hex.len() / HEX_GROUP);
    for (i, ch) in hex.chars().enumerate() {
        if i > 0 && i % HEX_GROUP == 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

// ---- 组织配置文件（线格式逐字见 ADR-071 / PRD-17 §13）----

/// 文件原样反序列化的形状。全字段必填（`tls_root_pem` 除外，见下）。
#[derive(Debug, Deserialize)]
struct RawOrgConfig {
    format_version: u32,
    kind: String,
    server_url: String,
    deployment_id: String,
    lease_pubkey: String,
    catalog_pubkey: String,
    /// **真可选**：反代根证书不在服务端手里（macOS 形态 Caddy 跑宿主）。导出方未配
    /// `TRUST_EXPORT_TLS_ROOT_PATH` 时该字段**如实缺席**——不造假字段、不塞空串，
    /// 客户端据此提示「需另行分发根证书」。这不是可选字段过渡（ADR-046 ④），
    /// 是一件真的可能不存在的东西。
    #[serde(default)]
    tls_root_pem: Option<String>,
    exported_at: String,
}

/// 校验通过的组织配置文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgConfig {
    /// 归一化后的服务端地址（去尾随 `/`）。
    pub server_url: String,
    pub deployment_id: String,
    pub lease_pubkey: String,
    pub catalog_pubkey: String,
    /// 根证书 PEM 原文；`None` = 文件里如实没有这一项。
    pub tls_root_pem: Option<String>,
    pub exported_at: String,
}

/// 文件被拒的原因。**每一条都必须指向一个用户能做的动作**——「解析失败」这类
/// 无动作可做的说法正是 ADR-060 起因里那句「指错方向的报错」的同类。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, thiserror::Error)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum OrgConfigRejection {
    /// 文件读不出来（路径不存在 / 无权限）。
    #[error("unreadable org config: {detail}")]
    Unreadable { detail: String },
    /// 不是 JSON，或缺少必填字段。
    #[error("malformed org config")]
    Malformed,
    /// JSON 能读，但 `kind` 不是组织配置文件（多半选错了文件）。
    #[error("not an org config file: {kind}")]
    NotOrgConfig { kind: String },
    /// 版本不认识 → **如实拒绝并提示升级客户端**，不猜、不兼容（ADR-046）。
    #[error("unsupported org config format version {found}")]
    UnsupportedFormatVersion { found: u32, supported: u32 },
    /// 地址不是可用的 origin（缺 scheme / 带路径）。
    #[error("bad server url")]
    BadServerUrl,
    /// 公钥不是 64 位 hex 或不是合法曲线点。`field` = `lease` / `catalog`。
    #[error("bad pubkey: {field}")]
    BadPubkey { field: String },
}

/// 解析 + 校验组织配置文件的字节。
///
/// # Errors
/// 见 [`OrgConfigRejection`]。**先判 `kind` 再判版本**：选错文件与版本太新是两条完全不同
/// 的排查路径，反过来判会让「随手选了个别的 json」被报成「请升级客户端」。
pub fn parse_org_config(bytes: &[u8]) -> Result<OrgConfig, OrgConfigRejection> {
    let raw: RawOrgConfig =
        serde_json::from_slice(bytes).map_err(|_| OrgConfigRejection::Malformed)?;
    if raw.kind != ORG_CONFIG_KIND {
        return Err(OrgConfigRejection::NotOrgConfig { kind: raw.kind });
    }
    if raw.format_version != ORG_CONFIG_FORMAT_VERSION {
        return Err(OrgConfigRejection::UnsupportedFormatVersion {
            found: raw.format_version,
            supported: ORG_CONFIG_FORMAT_VERSION,
        });
    }
    let server_url = raw.server_url.trim().trim_end_matches('/').to_owned();
    if !is_origin_url(&server_url) {
        return Err(OrgConfigRejection::BadServerUrl);
    }
    let lease_pubkey =
        normalized_pubkey(&raw.lease_pubkey).ok_or(OrgConfigRejection::BadPubkey {
            field: "lease".to_owned(),
        })?;
    let catalog_pubkey =
        normalized_pubkey(&raw.catalog_pubkey).ok_or(OrgConfigRejection::BadPubkey {
            field: "catalog".to_owned(),
        })?;
    Ok(OrgConfig {
        server_url,
        deployment_id: raw.deployment_id,
        lease_pubkey,
        catalog_pubkey,
        // 空串按"没有"处理：导出端不该塞空串，但真塞了的话，把它当成一份根证书
        // 引导用户去安装是更糟的结果。
        tls_root_pem: raw
            .tls_root_pem
            .map(|pem| pem.trim().to_owned())
            .filter(|pem| !pem.is_empty()),
        exported_at: raw.exported_at,
    })
}

/// 形状合格 → 小写 hex；否则 `None`。
///
/// 走 [`crate::config::parse_pubkey_hex`]（同一份判据）：形状对但不是合法曲线点的串
/// 必须在**导入**这一刻就被拒，而不是等到某次目录刷新报 `bad_signature`。
#[must_use]
pub fn normalized_pubkey(raw: &str) -> Option<String> {
    let trimmed = raw.trim().to_ascii_lowercase();
    crate::config::parse_pubkey_hex(&trimmed)?;
    Some(trimmed)
}

/// origin 形状：`scheme://host[:port]`，无路径/查询/片段。
fn is_origin_url(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once("://") else {
        return false;
    };
    if !matches!(scheme, "http" | "https") {
        return false;
    }
    !rest.is_empty() && !rest.contains(['/', '?', '#'])
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEASE: &str = "3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29";
    const CATALOG: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";

    /// 64 位 hex 形状、逐 seed 互不相同的测试串。
    ///
    /// 钉住与比对只做**逐字相等**（曲线点合法性在取钥/导入那一刻就已判过，见
    /// [`normalized_pubkey`]），故这里不需要真公钥。
    fn hex(seed: u8) -> String {
        format!("{seed:02x}").repeat(32)
    }

    fn org_config_json(overrides: serde_json::Value) -> Vec<u8> {
        let mut base = serde_json::json!({
            "format_version": 1,
            "kind": "lunarca-org-config",
            "server_url": "https://cp.internal:8443",
            "deployment_id": "dep-1",
            "lease_pubkey": LEASE,
            "catalog_pubkey": CATALOG,
            "exported_at": "2026-08-12T00:00:00Z",
        });
        let (serde_json::Value::Object(base_map), serde_json::Value::Object(over)) =
            (&mut base, overrides)
        else {
            unreachable!("both are objects");
        };
        for (k, v) in over {
            if v.is_null() {
                base_map.remove(&k);
            } else {
                base_map.insert(k, v);
            }
        }
        serde_json::to_vec(&base).unwrap()
    }

    #[test]
    fn a_well_formed_file_parses() {
        let cfg = parse_org_config(&org_config_json(serde_json::json!({}))).expect("合法文件");
        assert_eq!(cfg.server_url, "https://cp.internal:8443");
        assert_eq!(cfg.lease_pubkey, LEASE);
        assert!(
            cfg.tls_root_pem.is_none(),
            "文件里没有根证书时必须如实缺席——造一个空串出来，界面就会引导用户去安装一份不存在的证书"
        );
    }

    /// 尾随斜杠归一化：`https://h:8443/` 与 `https://h:8443` 必须落进**同一个桶**，
    /// 否则导入一次就会凭空多出一份钉住，而用户看到的是「刚导入完却说未钉住」。
    #[test]
    fn trailing_slash_is_normalized_into_the_same_bucket() {
        let cfg = parse_org_config(&org_config_json(
            serde_json::json!({ "server_url": "https://cp.internal:8443/" }),
        ))
        .expect("合法文件");
        assert_eq!(cfg.server_url, "https://cp.internal:8443");
        assert_eq!(
            crate::config::server_bucket(&cfg.server_url),
            crate::config::server_bucket("https://cp.internal:8443")
        );
    }

    /// `format_version != 1` → **如实拒绝并提示升级客户端**（ADR-046 的诚实拒绝语义）。
    ///
    /// 「多出来的字段大概不要紧」这条路走下去，就是拿一份读不全的信任锚去建立信任。
    #[test]
    fn an_unknown_format_version_is_refused_not_guessed() {
        let err = parse_org_config(&org_config_json(serde_json::json!({ "format_version": 2 })))
            .expect_err("版本不认识必须拒绝");
        assert_eq!(
            err,
            OrgConfigRejection::UnsupportedFormatVersion {
                found: 2,
                supported: 1
            }
        );
    }

    /// 选错文件（随手挑了个别的 json）必须说「这不是组织配置文件」，
    /// 而不是「请升级客户端」——两句话把人推向完全不同的方向。
    #[test]
    fn a_different_json_is_reported_as_the_wrong_file_not_a_version_problem() {
        let err = parse_org_config(&org_config_json(
            serde_json::json!({ "kind": "something-else", "format_version": 9 }),
        ))
        .expect_err("kind 不对必须拒绝");
        assert!(
            matches!(err, OrgConfigRejection::NotOrgConfig { .. }),
            "kind 判定必须排在版本判定前面，实得 {err:?}"
        );
    }

    #[test]
    fn malformed_and_bad_values_are_each_named() {
        assert_eq!(
            parse_org_config(b"not json").unwrap_err(),
            OrgConfigRejection::Malformed
        );
        assert_eq!(
            parse_org_config(&org_config_json(
                serde_json::json!({ "lease_pubkey": null })
            ))
            .unwrap_err(),
            OrgConfigRejection::Malformed,
            "缺必填字段与不是 JSON 同档：文件不完整，没有可执行的下一步差别"
        );
        assert_eq!(
            parse_org_config(&org_config_json(
                serde_json::json!({ "server_url": "cp.internal:8443" })
            ))
            .unwrap_err(),
            OrgConfigRejection::BadServerUrl,
            "缺 scheme 的地址必须当场拒——这正是 InvalidEndpoint 那条教训的入口侧"
        );
        assert_eq!(
            parse_org_config(&org_config_json(
                serde_json::json!({ "catalog_pubkey": "zz" })
            ))
            .unwrap_err(),
            OrgConfigRejection::BadPubkey {
                field: "catalog".to_owned()
            },
            "形状不对的公钥要在导入这一刻被拒，而不是等某次目录刷新报 bad_signature"
        );
    }

    /// **不发明短指纹**：分组只加空白，一个字符都不改、不截断。
    ///
    /// 服务端启动日志打印的就是完整 64 位 hex；自造短格式（截断 / 再散列 / 单词表）
    /// 会让用户手上那两串**根本对不上号**，而核对正是 TOFU 唯一的安全来源。
    #[test]
    fn grouping_never_shortens_or_alters_the_fingerprint() {
        let grouped = group_hex(LEASE);
        assert_eq!(
            grouped.chars().filter(|c| !c.is_whitespace()).count(),
            64,
            "分组后字符数变了——指纹被截断或改写了"
        );
        assert_eq!(
            grouped.replace(' ', ""),
            LEASE,
            "去掉空白必须逐字还原成服务端日志里那一串"
        );
        assert_eq!(&grouped[..9], "3b6a 27bc");
    }

    fn anchor(lease: &str, catalog: &str) -> PinnedAnchor {
        PinnedAnchor {
            lease_pubkey: lease.to_owned(),
            catalog_pubkey: catalog.to_owned(),
            pinned_at: 1000,
            provenance: TrustProvenance::Tofu,
            deployment_id: "dep-1".to_owned(),
        }
    }

    fn offer(lease: &str, catalog: &str, deployment: &str) -> OfferedAnchor {
        OfferedAnchor {
            lease_pubkey: lease.to_owned(),
            catalog_pubkey: catalog.to_owned(),
            deployment_id: deployment.to_owned(),
        }
    }

    /// **钉住的全部意义**：变更必须阻断，而比对绝不自动采用新钥。
    ///
    /// 自动采用 = 防回滚水位当场被架空——拿得到域名的攻击者换一把自己的钥，就能重签一份
    /// 任意旧版本的目录，而客户端会认为它「验签通过」。
    #[test]
    fn a_changed_anchor_blocks_and_never_repins_itself() {
        let store = TrustStore::new();
        let bucket = "cp.internal:8443";
        assert_eq!(
            store.record_offer(bucket, offer(LEASE, CATALOG, "dep-1")),
            TrustVerdict::Unpinned,
            "首连必须报未钉住（等一次确认），而不是静默钉上"
        );
        assert!(
            store.pinned(bucket).is_none(),
            "一次比对就把钥钉上了——那 TOFU 的「确认」这一步等于不存在"
        );

        store.pin(bucket, anchor(LEASE, CATALOG));
        assert_eq!(
            store.record_offer(bucket, offer(LEASE, CATALOG, "dep-1")),
            TrustVerdict::Match
        );

        // 服务端换了 catalog 那把钥（换钥的最小形态）。
        let rogue = "0000000000000000000000000000000000000000000000000000000000000000";
        assert_eq!(
            store.record_offer(bucket, offer(LEASE, rogue, "dep-1")),
            TrustVerdict::Changed,
            "只换一把钥也必须阻断——两把钥合起来才是这台服务端的身份"
        );
        assert_eq!(
            store.pinned(bucket).unwrap().catalog_pubkey,
            CATALOG,
            "阻断态下钉住值被改写了——确认前一律继续用旧钉住钥，否则防回滚水位形同虚设"
        );
        // 生效钥仍是钉住的那一组（本机没被动过），故判据落在「服务端换了钥」这一支。
        assert_eq!(store.verdict(bucket), TrustVerdict::Changed);
        assert_eq!(
            store.offered(bucket).unwrap().catalog_pubkey,
            rogue,
            "阻断横幅要并示新旧指纹，新的那一组必须留得住"
        );
    }

    /// `deployment_id` 变了但两把钥没变 —— **不是**信任锚变更。
    ///
    /// 它可以合法变更（重装、改名）。算进判据只会制造假阳性的红色横幅，而用户为了消掉
    /// 横幅会去点「确认」——把一次本该慎重的确认训练成一个反射动作，正是钉住最大的敌人。
    #[test]
    fn a_new_deployment_id_alone_is_not_a_trust_change() {
        let store = TrustStore::new();
        let bucket = "cp.internal:8443";
        store.pin(bucket, anchor(LEASE, CATALOG));
        assert_eq!(
            store.record_offer(bucket, offer(LEASE, CATALOG, "dep-renamed")),
            TrustVerdict::Match,
            "部署 id 变了就报「信任锚变了」——钥才是身份"
        );
    }

    /// 分桶：换服务器 = 换桶 = 重新建立信任；切回去仍认得。
    #[test]
    fn pinning_is_scoped_per_server_bucket() {
        let store = TrustStore::new();
        store.pin("a.internal:8443", anchor(LEASE, CATALOG));
        assert_eq!(store.verdict("a.internal:8443"), TrustVerdict::Match);
        assert_eq!(
            store.verdict("b.internal:8443"),
            TrustVerdict::Unpinned,
            "B 继承了 A 的钉住——那等于用 A 的身份给 B 背书"
        );
    }

    /// 钉住必须跨重启存活：它是用户做过的**一次安全决定**，不是可重建的缓存。
    /// 而「冲突观察」相反——它必须由再和服务端说一次话重新得出。
    #[test]
    fn pins_survive_restart_but_a_conflict_observation_does_not() {
        let dir = std::env::temp_dir().join(format!("luna-trust-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("trust.json");
        let _ = std::fs::remove_file(&path);

        let store = TrustStore::new();
        store.attach_path(path.clone());
        store.pin("cp.internal:8443", anchor(LEASE, CATALOG));
        let rogue = "0000000000000000000000000000000000000000000000000000000000000000";
        assert_eq!(
            store.record_offer("cp.internal:8443", offer(LEASE, rogue, "dep-1")),
            TrustVerdict::Changed
        );

        let reloaded = TrustStore::new();
        reloaded.attach_path(path);
        assert_eq!(
            reloaded
                .pinned("cp.internal:8443")
                .map(|a| a.catalog_pubkey),
            Some(CATALOG.to_owned()),
            "钉住没活过重启——用户下次启动会被要求重新确认一次他已经确认过的事"
        );
        assert!(
            reloaded.offered("cp.internal:8443").is_none(),
            "冲突观察被持久化了——一次网络抖动的误判会变成重启也消不掉、用户无法验证的红色横幅"
        );
    }

    /// **待确认的那一桶未必是当前配置的那一桶**（验收 review 抓出的死胡同）。
    ///
    /// 用户连着 A、导入的是 B 的组织配置文件（B 之前钉过、这次换了钥）：冲突记在 B 上，
    /// 而确认命令若按「当前桶」去找就一无所获——界面显示「需要确认」、命令回「没有待确认
    /// 的信任锚」，用户被卡在一个自相矛盾、走不通的路口（fail-closed，但那条路走不通）。
    #[test]
    fn a_pending_import_on_another_server_is_still_confirmable() {
        let store = TrustStore::new();
        store.pin("a.example.com", anchor(&hex(1), &hex(2)));
        store.pin("b.example.com", anchor(&hex(3), &hex(4)));

        // 连着 A，导入 B 的配置文件（B 换了钥）。
        let verdict = store.record_import_conflict(
            "b.example.com",
            offer(&hex(7), &hex(8), "dep-b"),
            "https://b.example.com",
        );
        assert_eq!(verdict, TrustVerdict::Changed, "换了钥必须判成变更");

        // 当前桶（A）自己没有待确认，故待确认的是 B——而不是 None。
        assert_eq!(
            store.pending_bucket("a.example.com").as_deref(),
            Some("b.example.com"),
            "导入冲突落在另一台上时找不到待确认项——确认按钮会点不动"
        );
        assert!(store.offered("b.example.com").is_some());
        assert_eq!(
            store.pending_import("b.example.com").as_deref(),
            Some("https://b.example.com"),
            "确认时要用文件里的地址落设置"
        );

        // 当前桶自己有待确认时，它优先（用户正看着的就是这一台）。
        store.record_offer("a.example.com", offer(&hex(9), &hex(10), "dep-a"));
        assert_eq!(
            store.pending_bucket("a.example.com").as_deref(),
            Some("a.example.com")
        );
    }

    /// 同一时刻至多一份待确认的导入：新的顶掉旧的，`pending_bucket` 因此永不需要在
    /// 多个候选之间猜（猜错 = 用户核对着 A 的指纹、确认下去的却是 B）。
    #[test]
    fn a_newer_import_supersedes_an_unconfirmed_one() {
        let store = TrustStore::new();
        store.pin("b.example.com", anchor(&hex(3), &hex(4)));
        store.pin("c.example.com", anchor(&hex(5), &hex(6)));
        store.record_import_conflict(
            "b.example.com",
            offer(&hex(7), &hex(8), "dep-b"),
            "https://b.example.com",
        );
        store.record_import_conflict(
            "c.example.com",
            offer(&hex(11), &hex(12), "dep-c"),
            "https://c.example.com",
        );
        assert!(
            store.pending_import("b.example.com").is_none(),
            "上一份未确认的导入必须被顶掉"
        );
        assert!(
            store.offered("b.example.com").is_none(),
            "被顶掉的观察一并清掉，否则 pending_bucket 面对多个候选"
        );
        assert_eq!(
            store.pending_bucket("a.example.com").as_deref(),
            Some("c.example.com")
        );
    }

    /// **客户端 HTTP 栈里不得存在任何「跳过 TLS 校验」的路径**（修 1）。
    ///
    /// TOFU 副路径的前提是 TLS 已可信（根证书已装或用公网可信证书）。给一个
    /// 「先跳过校验连上再说」的开关，等于把 ADR-060 ② 里唯一真能抵抗中间人的那一层
    /// 直接关掉——而它一旦存在，就一定会在某次"临时排障"里被打开并留在那儿（HPKP
    /// 时代的教训）。故连**能被打开的形状**都不许有：没有 env、没有设置项、没有构建特性。
    ///
    /// 唯一的例外是 `first_contact.rs`（ADR-158 ④）：未钉住的服务端上，放宽校验的客户端只发
    /// OPAQUE 两段与两条只带邮箱的申请码请求，握手结果当场用会话密钥解开信任包并核对部署——
    /// 信任来自口令，不来自那条 TLS 连接；不是「先连上再说」的开关。
    ///
    /// 扫的是整棵工作树的 Rust 源码而不是只扫本 crate：出网的 HTTP 栈不止一处
    /// （`luna-provider` 的推理面、`src-tauri` 的制品下载），只守住这里等于没守。
    /// `first_contact.rs` 里出现的每一个 `"/v1/…"` 字面量都必须是允许放宽的四条之一。
    fn first_contact_paths_are_allowed(source: &str) -> bool {
        let mut found = 0_usize;
        for (index, _) in source.match_indices("\"/v1/") {
            let rest = &source[index + 1..];
            let Some(end) = rest.find('"') else {
                return false;
            };
            if !crate::first_contact::ALLOWED_PATHS.contains(&&rest[..end]) {
                return false;
            }
            found += 1;
        }
        found == crate::first_contact::ALLOWED_PATHS.len()
    }

    #[test]
    fn the_first_contact_exemption_rejects_any_other_path() {
        let source = include_str!("first_contact.rs");
        assert!(first_contact_paths_are_allowed(source));
        let widened = format!(
            "{source}
const X: &str = \"/v1/me\";
"
        );
        assert!(!first_contact_paths_are_allowed(&widened));
    }

    #[test]
    fn no_code_path_can_ever_skip_tls_verification() {
        const FORBIDDEN: &[&str] = &[
            "danger_accept_invalid_certs",
            "danger_accept_invalid_hostnames",
            "accept_invalid_certs",
            "accept_invalid_hostnames",
        ];
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let mut scanned = 0_usize;
        let mut offenders = Vec::new();
        let mut stack = vec![root.join("crates"), root.join("src-tauri").join("src")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // 生成物与依赖源码不是我们的代码，扫它们只会制造假阳性。
                    if !matches!(
                        path.file_name().and_then(|n| n.to_str()),
                        Some("target" | "node_modules")
                    ) {
                        stack.push(path);
                    }
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(source) = std::fs::read_to_string(&path) else {
                    continue;
                };
                scanned += 1;
                // 本用例自己那份禁词表当然会命中，跳过它。
                if path.ends_with("luna-cloud/src/trust.rs") {
                    continue;
                }
                // 唯一豁免（ADR-158 ④）：首连握手的放宽客户端。只许出现一次；同一文件里必须有
                // 「信任包用会话密钥解开并核对部署」那两句；文件里出现的每一条 `/v1/` 路径都必须在
                // 允许的四条之内——多一处放宽、多发一条路径、或核对被删掉，照样算违规。
                let exempt = path.ends_with("luna-cloud/src/first_contact.rs")
                    && source.matches("danger_accept_invalid_certs(true)").count() == 1
                    && source.contains("opaque_auth::open_bundle(")
                    && source.contains("opaque_auth::ensure_same_deployment(")
                    && first_contact_paths_are_allowed(&source);
                for needle in FORBIDDEN {
                    let allowed = usize::from(exempt && needle.contains("accept_invalid_certs"));
                    if source.matches(needle).count() > allowed {
                        offenders.push(format!("{}: {needle}", path.display()));
                    }
                }
            }
        }
        assert!(
            scanned > 50,
            "只扫到 {scanned} 个 .rs 文件——这条守护空跑了，它挡不住任何东西"
        );
        assert!(
            offenders.is_empty(),
            "出现了跳过 TLS 校验的路径：{offenders:?}\n             \
             TOFU 钉的是签名公钥，TLS 那一层的信任来自系统信任库里的根证书（ADR-060 ②）。\
             关掉校验 = 首连窗口对中间人完全敞开，而钉住的两把钥会把攻击者的身份如实钉住。"
        );
    }
}

// ── 轮换链（ADR-124）────────────────────────────────────────────────────────

/// 一条服务端下发的轮换声明。
///
/// `statement` 是 base64url 的规范 JSON 字节，`signature` 是**上一代 lease 私钥**对
/// **那串字节**的 Ed25519 签名。两端都不重新序列化——否则「两端 JSON 逐字节一致」
/// 变成一条隐式契约，而它破掉时的表现是全部客户端同时验签失败。
#[derive(Debug, Clone, Deserialize)]
pub struct RotationStatement {
    pub statement: String,
    pub signature: String,
}

/// 一次最多沿多少条声明走。与服务端那一侧**成对声明**（ADR-124 ⑧）：不封顶的话，
/// 一个被塞满的数组就是一条免鉴权的放大面，而这一侧是逐条验签、每条一次 Ed25519。
pub const MAX_ROTATION_STATEMENTS: usize = 64;

/// 声明里的内容（只解出判定要用的那些字段）。
#[derive(Debug, Deserialize)]
struct RotationPayload {
    kind: String,
    format_version: u32,
    seq: i64,
    previous: RotationKeys,
    next: RotationKeys,
}

#[derive(Debug, Deserialize)]
struct RotationKeys {
    lease: String,
    catalog: String,
}

/// 声明的 `kind` 判别字段（逐字契约，与服务端对齐）。
const ROTATION_KIND: &str = "lunarca-key-rotation";
/// 本客户端支持的声明格式版本。不认识即不验——**退回人眼核对**，不猜。
const ROTATION_FORMAT_VERSION: u32 = 1;

/// 从**已钉住的那一代**沿链走，能不能验证到服务端此刻自报的这一组钥？
///
/// 验得通 = 这次换钥由上一代私钥背书过，可以自动重钉（照走三重重置）。验不通 = 原样
/// 退回 [`TrustVerdict::Changed`] 的阻断与人眼核对。**它只能把一次本来就要打断用户的事
/// 变得不打断，永远不能让一次本来会被拦下的变更通过**——这条是本函数的安全底，
/// 每一条 `return false` 都在守它。
///
/// # 为什么不另记一份「已撤销的旧钥」
///
/// ADR-124 ④ 初稿要求把链上每个 `previous`
/// 记进撤销集合。落地时发现它**不多防住任何东西**：链必须从**当前钉住的那一代**起步，
/// 于是一条旧声明的 `previous` 根本对不上钉住值，重放当场失败；而如果攻击者手上有的
/// 正是当前钉住那一代的私钥，撤销集合也救不了——那一代还没被撤销。故不实现，
/// 并在 ADR 上记了这条订正。
#[must_use]
pub fn follow_rotation_chain(
    pinned: &PinnedAnchor,
    offered: &OfferedAnchor,
    statements: &[RotationStatement],
) -> bool {
    if statements.is_empty() || statements.len() > MAX_ROTATION_STATEMENTS {
        return false;
    }
    let mut lease = pinned.lease_pubkey.clone();
    let mut catalog = pinned.catalog_pubkey.clone();
    let mut last_seq: Option<i64> = None;
    // 还没接上自己那一代之前，验不过的声明是**更早的几代**，跳过；接上之后再验不过就是断链。
    //
    // 服务端下发的是**完整链**（ADR-124 ⑧：钉在更早一代的客户端才接得上），于是一个已经
    // 跟过一次轮换的客户端，看到的第一条必然是它出生之前那一代的——把「链首必须是我这一代」
    // 当成硬判据，第一次轮换能过、**第二次起永远过不去**，而第二次才是生产上的常态
    // （真机 2026-09-11 第二次轮换当场撞上）。
    //
    // 跳过的判据是**签名验不过**，不是去读一段还没验过的 JSON 说它是哪一代：攻击者可以在
    // 链前面塞任意垃圾，那些垃圾一律验不过、一律被跳过，而他伪造不出一条由**我钉住的那把
    // 私钥**签出来的声明——「只接受我这一代签的东西」这条底线一个字都没松。
    let mut started = false;
    for entry in statements {
        let Some(payload) = verified_payload(entry, &lease) else {
            if started {
                return false;
            }
            continue;
        };
        if payload.kind != ROTATION_KIND || payload.format_version != ROTATION_FORMAT_VERSION {
            return false;
        }
        // 验过了，它就必须自称从我这一代出发——签得动我的钥、却说自己接在别处，是自相矛盾。
        if payload.previous.lease != lease || payload.previous.catalog != catalog {
            return false;
        }
        started = true;
        // `seq` 严格递增：不递增就意味着链里有重复或回环，而沿一个回环走下去的客户端
        // 最终落在哪一代取决于顺序——那不该是实现细节。
        if last_seq.is_some_and(|previous| payload.seq <= previous) {
            return false;
        }
        last_seq = Some(payload.seq);
        let (Some(next_lease), Some(next_catalog)) = (
            normalized_pubkey(&payload.next.lease),
            normalized_pubkey(&payload.next.catalog),
        ) else {
            return false;
        };
        lease = next_lease;
        catalog = next_catalog;
    }
    // 一条都没接上 = 这条链里没有我那一代，退回人眼核对。
    // 链尾必须**逐字**等于服务端此刻自报的那一组。差一个字节都退回人眼核对。
    started && lease == offered.lease_pubkey && catalog == offered.catalog_pubkey
}

/// 验签并解出内容；任何一步不成立都回 `None`（调用方一律据此退回阻断）。
fn verified_payload(entry: &RotationStatement, signer_lease_hex: &str) -> Option<RotationPayload> {
    use base64::Engine as _;
    use ed25519_dalek::Verifier as _;

    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&entry.statement)
        .ok()?;
    let signature_raw = decode_hex(&entry.signature)?;
    let signature = ed25519_dalek::Signature::from_slice(&signature_raw).ok()?;
    let key_raw = decode_hex(signer_lease_hex)?;
    let key_bytes: [u8; 32] = key_raw.try_into().ok()?;
    let key = ed25519_dalek::VerifyingKey::from_bytes(&key_bytes).ok()?;
    key.verify(&bytes, &signature).ok()?;
    // **先验签再解析**：解析一段没验过的 JSON 只会让「格式不对」和「签名不对」两种
    // 失败混在一起，而后者是安全事实、前者不是。
    serde_json::from_slice(&bytes).ok()
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) || !hex.is_ascii() {
        return None;
    }
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let hi = (pair[0] as char).to_digit(16)?;
            let lo = (pair[1] as char).to_digit(16)?;
            u8::try_from((hi << 4) | lo).ok()
        })
        .collect()
}

/// 轮换链的测试夹具——**一份**，`client.rs` 那边的用例也用它。
///
/// 造一条声明要与服务端逐字同形（签名覆盖下发的那串字节）。两处各写一份的话，其中一份
/// 迟早与真实线格式分叉，而分叉的表现是「测试全绿、真机验不过」。
#[cfg(test)]
pub(crate) mod rotation_fixtures {
    use super::*;
    use ed25519_dalek::{Signer as _, SigningKey};

    pub(crate) fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    pub(crate) fn hex(bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        bytes.iter().fold(String::new(), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
    }

    pub(crate) fn pubkey(k: &SigningKey) -> String {
        hex(&k.verifying_key().to_bytes())
    }

    /// 按服务端那一侧的形状造一条声明：签名覆盖的就是下发的那串字节。
    pub(crate) fn statement(
        signer: &SigningKey,
        seq: i64,
        previous: (&SigningKey, &SigningKey),
        next: (&SigningKey, &SigningKey),
    ) -> RotationStatement {
        use base64::Engine as _;
        let payload = serde_json::json!({
            "kind": "lunarca-key-rotation",
            "format_version": 1,
            "deployment_id": "acme",
            "seq": seq,
            "previous": { "lease": pubkey(previous.0), "catalog": pubkey(previous.1) },
            "next": { "lease": pubkey(next.0), "catalog": pubkey(next.1) },
            "issued_at": 1_757_000_000,
        })
        .to_string()
        .into_bytes();
        RotationStatement {
            statement: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&payload),
            signature: hex(&signer.sign(&payload).to_bytes()),
        }
    }

    pub(crate) fn pinned_at(lease: &SigningKey, catalog: &SigningKey) -> PinnedAnchor {
        PinnedAnchor {
            lease_pubkey: pubkey(lease),
            catalog_pubkey: pubkey(catalog),
            pinned_at: 0,
            provenance: TrustProvenance::Tofu,
            deployment_id: "acme".to_owned(),
        }
    }

    pub(crate) fn offered_at(lease: &SigningKey, catalog: &SigningKey) -> OfferedAnchor {
        OfferedAnchor {
            lease_pubkey: pubkey(lease),
            catalog_pubkey: pubkey(catalog),
            deployment_id: "acme".to_owned(),
        }
    }
}

#[cfg(test)]
mod rotation_chain_tests {
    use super::rotation_fixtures::*;
    use super::*;
    use ed25519_dalek::Signer as _;

    #[test]
    fn one_step_signed_by_the_pinned_generation_is_accepted() {
        let (l1, c1, l2, c2) = (key(1), key(2), key(3), key(4));
        let chain = [statement(&l1, 1, (&l1, &c1), (&l2, &c2))];
        assert!(follow_rotation_chain(
            &pinned_at(&l1, &c1),
            &offered_at(&l2, &c2),
            &chain
        ));
    }

    /// 两步：钉在第一代的客户端也要接得上——只发最新一条的服务端等于没做轮换。
    #[test]
    fn two_steps_each_signed_by_its_own_generation_are_accepted() {
        let (l1, c1, l2, c2, l3, c3) = (key(1), key(2), key(3), key(4), key(5), key(6));
        let chain = [
            statement(&l1, 1, (&l1, &c1), (&l2, &c2)),
            statement(&l2, 2, (&l2, &c2), (&l3, &c3)),
        ];
        assert!(follow_rotation_chain(
            &pinned_at(&l1, &c1),
            &offered_at(&l3, &c3),
            &chain
        ));
    }

    /// **钉在链中间**的客户端要从自己那一代接着走——这是生产上的常态。
    ///
    /// 服务端下发完整链（ADR-124 ⑧），于是一个已经跟过一次轮换的客户端看到的第一条，
    /// 必然是它出生之前那一代的。把「链首必须是我这一代」当硬判据的话，第一次轮换能过、
    /// **第二次起永远过不去**——真机 2026-09-11 第二次轮换当场撞上，而在此之前
    /// 本文件、契约测试、真机第一轮**全都只覆盖了钉在链首那一种**。
    #[test]
    fn a_client_pinned_mid_chain_follows_from_its_own_generation() {
        let (l1, c1, l2, c2, l3, c3) = (key(1), key(2), key(3), key(4), key(5), key(6));
        let chain = [
            statement(&l1, 1, (&l1, &c1), (&l2, &c2)),
            statement(&l2, 2, (&l2, &c2), (&l3, &c3)),
        ];
        assert!(
            follow_rotation_chain(&pinned_at(&l2, &c2), &offered_at(&l3, &c3), &chain),
            "钉在第二代的客户端必须跳过第一条、从自己那一代接着走"
        );
    }

    /// 跳过是**验签驱动**的，不是读一段没验过的 JSON：链前面塞任意垃圾都跳得过去，
    /// 而垃圾里没有一条是我钉住那把私钥签的——「只接受我这一代签的东西」没有松动。
    #[test]
    fn junk_before_my_generation_is_skipped_not_trusted() {
        let (l1, c1, l2, c2, l3, c3) = (key(1), key(2), key(3), key(4), key(5), key(6));
        let (lx, cx) = (key(21), key(22));
        let chain = [
            // 攻击者自签的一条，声称把 gen2 换到他手上那把。
            statement(&lx, 99, (&l2, &c2), (&lx, &cx)),
            statement(&l1, 1, (&l1, &c1), (&l2, &c2)),
            statement(&l2, 2, (&l2, &c2), (&l3, &c3)),
        ];
        // 跟到的是真链尾 gen3，不是攻击者那把。
        assert!(follow_rotation_chain(
            &pinned_at(&l2, &c2),
            &offered_at(&l3, &c3),
            &chain
        ));
        assert!(
            !follow_rotation_chain(&pinned_at(&l2, &c2), &offered_at(&lx, &cx), &chain),
            "自签的那条被当真了——攻击者只要在链前面塞一条就能把客户端带走"
        );
    }

    /// 接上之后再断就是断链，不能继续「跳过」——否则链中间缺一节也会被走通。
    #[test]
    fn a_gap_after_joining_the_chain_is_not_skipped() {
        let (l2, c2, l3, c3, l5, c5) = (key(3), key(4), key(5), key(6), key(9), key(10));
        let (l4, c4) = (key(7), key(8));
        let chain = [
            statement(&l2, 2, (&l2, &c2), (&l3, &c3)),
            // 缺了 gen3 → gen4 那一节，这条由 gen4 签、gen3 验不过。
            statement(&l4, 4, (&l4, &c4), (&l5, &c5)),
        ];
        assert!(!follow_rotation_chain(
            &pinned_at(&l2, &c2),
            &offered_at(&l5, &c5),
            &chain
        ));
    }

    /// 第二条用**第一代**的钥签 = 上一代根本没背书这次换钥。这正是本函数要拦的那件事：
    /// 攻击者手上若有一条 gen1 签的声明，不该能拿它把 gen2 的客户端带到任意一代去。
    #[test]
    fn a_step_signed_by_the_wrong_generation_is_refused() {
        let (l1, c1, l2, c2, l3, c3) = (key(1), key(2), key(3), key(4), key(5), key(6));
        let chain = [
            statement(&l1, 1, (&l1, &c1), (&l2, &c2)),
            statement(&l1, 2, (&l2, &c2), (&l3, &c3)),
        ];
        assert!(!follow_rotation_chain(
            &pinned_at(&l1, &c1),
            &offered_at(&l3, &c3),
            &chain
        ));
    }

    #[test]
    fn a_tampered_signature_is_refused() {
        let (l1, c1, l2, c2) = (key(1), key(2), key(3), key(4));
        let mut chain = [statement(&l1, 1, (&l1, &c1), (&l2, &c2))];
        chain[0].signature.replace_range(0..2, "ff");
        assert!(!follow_rotation_chain(
            &pinned_at(&l1, &c1),
            &offered_at(&l2, &c2),
            &chain
        ));
    }

    /// 链首接不上钉住值：这条链讲的不是「我这台」的故事。
    #[test]
    fn a_chain_that_does_not_start_at_the_pinned_generation_is_refused() {
        let (l1, c1, l2, c2, lx, cx) = (key(1), key(2), key(3), key(4), key(7), key(8));
        let chain = [statement(&lx, 1, (&lx, &cx), (&l2, &c2))];
        assert!(!follow_rotation_chain(
            &pinned_at(&l1, &c1),
            &offered_at(&l2, &c2),
            &chain
        ));
    }

    /// 链尾与服务端自报的那一组差一个字节 → 退回人眼核对。
    ///
    /// 不查这一条的后果最隐蔽：链本身条条验得过，而客户端最终钉住的是链尾那一代，
    /// 服务端却在用另一把钥签目录——表现是「已核验」与「签名校验失败」同时成立。
    #[test]
    fn a_chain_that_ends_somewhere_else_is_refused() {
        let (l1, c1, l2, c2, lz, cz) = (key(1), key(2), key(3), key(4), key(9), key(10));
        let chain = [statement(&l1, 1, (&l1, &c1), (&l2, &c2))];
        assert!(!follow_rotation_chain(
            &pinned_at(&l1, &c1),
            &offered_at(&lz, &cz),
            &chain
        ));
    }

    #[test]
    fn a_non_increasing_seq_is_refused() {
        let (l1, c1, l2, c2, l3, c3) = (key(1), key(2), key(3), key(4), key(5), key(6));
        let chain = [
            statement(&l1, 2, (&l1, &c1), (&l2, &c2)),
            statement(&l2, 2, (&l2, &c2), (&l3, &c3)),
        ];
        assert!(!follow_rotation_chain(
            &pinned_at(&l1, &c1),
            &offered_at(&l3, &c3),
            &chain
        ));
    }

    /// 认不出的 `kind` / 版本 → 不验，退回人眼核对（ADR-046 的诚实拒绝语义，不猜）。
    #[test]
    fn an_unknown_kind_or_version_is_refused() {
        use base64::Engine as _;
        let (l1, c1, l2, c2) = (key(1), key(2), key(3), key(4));
        for payload in [
            serde_json::json!({ "kind": "something-else", "format_version": 1, "seq": 1,
                "previous": { "lease": pubkey(&l1), "catalog": pubkey(&c1) },
                "next": { "lease": pubkey(&l2), "catalog": pubkey(&c2) } }),
            serde_json::json!({ "kind": "lunarca-key-rotation", "format_version": 2, "seq": 1,
                "previous": { "lease": pubkey(&l1), "catalog": pubkey(&c1) },
                "next": { "lease": pubkey(&l2), "catalog": pubkey(&c2) } }),
        ] {
            let bytes = payload.to_string().into_bytes();
            let chain = [RotationStatement {
                statement: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes),
                signature: hex(&l1.sign(&bytes).to_bytes()),
            }];
            assert!(!follow_rotation_chain(
                &pinned_at(&l1, &c1),
                &offered_at(&l2, &c2),
                &chain
            ));
        }
    }

    /// 空链与超上限都不是「验过了」。空链尤其要拦——服务端根本没下发轮换时，
    /// 一个「零条也算走到了」的实现会让任何一次换钥都被自动接受。
    #[test]
    fn an_empty_or_oversized_chain_is_refused() {
        let (l1, c1) = (key(1), key(2));
        assert!(!follow_rotation_chain(
            &pinned_at(&l1, &c1),
            &offered_at(&l1, &c1),
            &[]
        ));
        let one = statement(&l1, 1, (&l1, &c1), (&l1, &c1));
        let oversized: Vec<RotationStatement> = std::iter::repeat_with(|| one.clone())
            .take(MAX_ROTATION_STATEMENTS + 1)
            .collect();
        assert!(!follow_rotation_chain(
            &pinned_at(&l1, &c1),
            &offered_at(&l1, &c1),
            &oversized
        ));
    }
}
