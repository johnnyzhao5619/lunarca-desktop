//! 实时冒烟：对着**真 Lunarca 服务端**做协议版本协商。默认 `#[ignore]`。
//!
//! 补的是「协商逻辑 ↔ 服务端响应」那道缝。协商本身有纯函数单测（五条分支全覆盖），
//! 服务端也有自己的用例，但真正决定用户看到什么的是这段**真实 HTTP 往返**：字段名一改、
//! 类型一变（`protocol` 从 number 变 string 之类），两侧单测照样绿，而 GUI 上就是
//! 「暂时取不到服务端状态」——恰好是这个特性存在的目的（消灭神秘故障）反过来自己变神秘。
//!
//! ```sh
//! LUNA_CLOUD_API_BASE=http://10.0.0.62:8090 \
//! LUNA_CLOUD_ACCESS_TOKEN=... \
//!   cargo test -p luna-cloud --test service_live -- --ignored --nocapture
//! ```

use luna_cloud::artifact::ArtifactClient;
use luna_cloud::catalog::{CatalogClient, CatalogClientState, CatalogKind};
use luna_cloud::client::CloudClient;
use luna_cloud::config::parse_pubkey_hex;
use luna_cloud::protocol::CLIENT_PROTOCOL;
use luna_cloud::store::{CredentialStore, MemoryStore, KEY_ACCESS, KEY_REFRESH};

/// 目录观察者交出来的那一份（版本 + 条目）。命名而不是就地写全，是 clippy 的
/// `type_complexity` 要求，也让下面那条断言读得出它在等什么。
type SeenCatalog =
    std::sync::Arc<std::sync::Mutex<Option<luna_cloud::CatalogDelivery<serde_json::Value>>>>;

/// 提示词目录那一份（同上，条目类型不同）。
type SeenPrompts = std::sync::Arc<
    std::sync::Mutex<Option<luna_cloud::CatalogDelivery<luna_core::CapabilityPromptSet>>>,
>;

/// 按 `CloudClient` **真正读取的方式**写入会话凭据。
///
/// **桶名必须取自 `config::server_bucket`，不能写裸键**——这条是真机抓出来的
/// （2026-08-11 全清白盒轮）。客户端的凭据一律按服务端分桶存取（`<bucket>/<key>`，见
/// `client::scoped`），而这里此前写的是裸 `KEY_ACCESS` / `KEY_REFRESH`。于是 `authed`
/// 取不到 refresh，在发出任何字节之前短路成 `NotLoggedIn`：
/// `service_models_lists_real_server_catalog` **自「凭据按服务端分桶」落地起就不可能通过**，
/// 而它带 `#[ignore]`、只在有人手动指向真服务端时才跑，失败又极易被读成「环境没配好」——
/// 这正是这类缺陷能活下来的原因。同一处还让隔壁用例里「带着会话时也走得通」那句注释
/// 成了空话：那两个 token 从来没被客户端看见过。
fn store_with_session(api_base: &str, access: &str, refresh: &str) -> MemoryStore {
    let bucket = luna_cloud::config::server_bucket(api_base);
    let store = MemoryStore::new();
    store
        .set(&format!("{bucket}/{KEY_ACCESS}"), access)
        .unwrap();
    store
        .set(&format!("{bucket}/{KEY_REFRESH}"), refresh)
        .unwrap();
    store
}

#[tokio::test]
#[ignore = "需要可达的 Lunarca 服务端与有效 access token"]
async fn service_status_negotiates_against_real_server() {
    let api_base = std::env::var("LUNA_CLOUD_API_BASE").expect("缺 LUNA_CLOUD_API_BASE");
    let token = std::env::var("LUNA_CLOUD_ACCESS_TOKEN").expect("缺 LUNA_CLOUD_ACCESS_TOKEN");
    let refresh = std::env::var("LUNA_CLOUD_REFRESH_TOKEN").expect("缺 LUNA_CLOUD_REFRESH_TOKEN");

    // refresh 也要塞：`authed` 在**无 refresh token 时本地短路**返回 NotLoggedIn，
    // 一个字节都不发——那是「未登录零云网络行为」不变量的实现方式，不是 bug。
    // 只塞 access 会让这条用例以 NotLoggedIn 失败，而它测的根本不是登录态。
    let store = store_with_session(api_base.trim_end_matches('/'), &token, &refresh);
    let client = {
        let client = CloudClient::new(Box::new(store), std::sync::Arc::new(|| false));
        client
            .account()
            .set_server(luna_cloud::account::AccountServer {
                origin: api_base.trim_end_matches('/').to_owned(),
                display_name: None,
            });
        client.reconfigure();
        client
    };

    // 匿名取 capabilities 并协商（登录前就要知道登录方式）；协议不兼容即返回错误。
    let caps = client
        .fetch_capabilities_at(api_base.trim_end_matches('/'))
        .await
        .expect("协商请求应成功");
    println!(
        "服务端 protocol={} min_client={} / 客户端 {CLIENT_PROTOCOL}；登录方式 {:?}",
        caps.protocol, caps.min_client_protocol, caps.auth.login
    );
    assert!(
        caps.protocol >= CLIENT_PROTOCOL,
        "服务端版本解析失败会得到 0——那说明字段名或类型变了"
    );
}

/// 服务端模型清单的真实往返（同一环境变量组，同样默认 `#[ignore]`）。
///
/// 这条钉的是**用户能不能选到型**：客户端把分配槽位里的模型名原样发给端点，而
/// 「服务端到底有哪些名字」只能从这个接口得到。字段名一改（`model` → `id` 之类）
/// 客户端拿到的就是一份空清单——界面上表现为「服务端没有可用模型」，与
/// 管理员真的没登记模型**完全同形**，用户与运维都无从分辨。
#[tokio::test]
#[ignore = "需要可达的 Lunarca 服务端与有效 access token"]
async fn service_models_lists_real_server_catalog() {
    let api_base = std::env::var("LUNA_CLOUD_API_BASE").expect("缺 LUNA_CLOUD_API_BASE");
    let token = std::env::var("LUNA_CLOUD_ACCESS_TOKEN").expect("缺 LUNA_CLOUD_ACCESS_TOKEN");
    let refresh = std::env::var("LUNA_CLOUD_REFRESH_TOKEN").expect("缺 LUNA_CLOUD_REFRESH_TOKEN");

    let store = store_with_session(api_base.trim_end_matches('/'), &token, &refresh);
    let client = {
        let client = CloudClient::new(Box::new(store), std::sync::Arc::new(|| false));
        client
            .account()
            .set_server(luna_cloud::account::AccountServer {
                origin: api_base.trim_end_matches('/').to_owned(),
                display_name: None,
            });
        client.reconfigure();
        client
    };

    let models = client.service_models().await.expect("模型清单请求应成功");
    for model in &models {
        println!(
            "{} ({}) 档位={:?} 能力={:?} 窗口={:?}",
            model.display_name,
            model.model,
            model.price_tier,
            model.capabilities,
            model.context_window
        );
    }
    assert!(
        !models.is_empty(),
        "清单为空：要么服务端确实没登记模型，要么字段名变了导致整份解析成空——         两者在界面上同形，这里必须由真机把它们分开"
    );
    assert!(
        models.iter().all(|model| !model.model.trim().is_empty()),
        "model 串是发给端点的标识，空串等于选不了型"
    );
    // **`price_tier` 恒为 None，这是裁决不是缺陷**（[ADR-039]）：计费由版本化费率表与
    // 服务端账本裁决，一个会随服务端路由变化、用户又控制不了的「档位」字符串不进客户端。
    //
    // 这条断言此前写的是它的**反面**（「至少一个模型应带计费档位」）——那是 ADR-039 之前的
    // 需求，废止后没人回来改，因为它一直够不着：上面的分桶缺陷让请求在此之前就失败了。
    // 现在反过来钉住废止本身：谁把 price_tier 重新填上，这里就会红，并被指回 ADR-039。
    assert!(
        models.iter().all(|model| model.price_tier.is_none()),
        "price_tier 已由 ADR-039 废止，服务端不应再下发它——真要恢复计费档位，先改 ADR"
    );
}

/// 匿名访问必须被拒。
///
/// 模型清单看似无害，实则是**部署方的服务目录**：谁在提供什么能力、按什么档计费。
/// 无凭据可读等于把它公开给任何能连上端口的人。这条与「未登录零云网络行为」是两件事
/// ——那条管客户端不主动发，这条管服务端不白给。
#[tokio::test]
#[ignore = "需要可达的 Lunarca 服务端"]
async fn service_models_rejects_anonymous() {
    let api_base = std::env::var("LUNA_CLOUD_API_BASE").expect("缺 LUNA_CLOUD_API_BASE");
    let response = reqwest::Client::new()
        .get(format!(
            "{}/inference/v1/models",
            api_base.trim_end_matches('/')
        ))
        .send()
        .await
        .expect("请求应发出");
    assert_eq!(
        response.status().as_u16(),
        401,
        "匿名请求必须 401；其它状态码都意味着服务目录对外泄露或路由错挂"
    );
}

/// 目录发布 → 客户端验签 → 制品面 resolve → **真的把字节拉下来并终验 sha256**。
///
/// 这条补的是纵切 1 唯一没被任何机器守护盯住的那一段。离线契约测试（商业仓
/// `contract-tests/tests/catalog_flow.rs`）用的是进程内 cp-api 与自建的假制品站点：
/// 它证明编排正确，但**证明不了那份真实目录里的 9 条制品当真拿得到**——URL 搬家、
/// 上游改成需要登录、sha256 与实际文件对不上，三者在离线用例里全绿，而在用户机器上
/// 是「模型装不上」。判据必须落在真实字节上。
///
/// 需要：一台已激活**本地模型目录**（`local_models` 类别，`/v1/catalog/local-models`，ADR-145）
/// 的服务端 + 它的**目录根公钥**（ADR-169 ⑥，不是在线目录钥）。服务端尚未发布这一类别时，本条会以 `no_active_catalog`
/// 失败——那是如实的，不是本条写错了。
///
/// ```sh
/// LUNA_CLOUD_API_BASE=https://10.0.0.62:8443 \
/// LUNA_CLOUD_CATALOG_PUBKEY=<64 hex> \
///   cargo test -p luna-cloud --test service_live -- --ignored --nocapture \
///     catalog_publish_reaches_the_client
/// ```
#[tokio::test]
#[ignore = "需要可达的 Lunarca 服务端与已激活的模型目录"]
async fn catalog_publish_reaches_the_client_and_artifact_bytes_land() {
    let api_base = std::env::var("LUNA_CLOUD_API_BASE").expect("缺 LUNA_CLOUD_API_BASE");
    let api_base = api_base.trim_end_matches('/').to_owned();
    let pubkey = parse_pubkey_hex(
        &std::env::var("LUNA_CLOUD_CATALOG_PUBKEY").expect("缺 LUNA_CLOUD_CATALOG_PUBKEY"),
    )
    .expect("目录根公钥必须是 64 位十六进制");

    let seen: SeenCatalog = std::sync::Arc::new(std::sync::Mutex::new(None));
    let sink = seen.clone();
    let catalog = CatalogClient::<serde_json::Value>::new(
        CatalogKind::LocalModels,
        api_base.clone(),
        Some(pubkey),
        std::sync::Arc::new(|| false),
    )
    .with_observer(std::sync::Arc::new(move |payload| {
        *sink.lock().expect("observer sink poisoned") = payload;
    }));
    // 状态文件放临时目录并带进程号：本条用例绝不复用真实用户的防回滚水位——
    // 复用了的话，本机曾验过更高版本就会让这条用例以 `rollback` 失败，而那与
    // 服务端毫无关系。
    catalog.attach_state_path(
        std::env::temp_dir().join(format!("lunarca-live-catalog-{}.json", std::process::id())),
    );

    let status = catalog.refresh().await.expect("目录刷新请求应成功");
    assert_eq!(
        status.state,
        CatalogClientState::Verified,
        "目录未验签通过（last_error={:?}）——`no_active_catalog` 说明服务端还没走完 \
         Draft→Reviewed→Signed→Staged→Active；`bad_keys` 说明钉的不是这台部署的根公钥\
         （常见是错填成在线目录钥），`keys_expired` 说明该跑 `lunarcactl catalog-keys renew`",
        status.last_error
    );
    let version = status.verified_version.expect("已验签目录必须报得出版本号");
    assert!(version >= 1, "目录版本从 1 起，0 说明字段没解析出来");

    let luna_cloud::CatalogDelivery {
        version: observed_version,
        entries,
        ..
    } = seen
        .lock()
        .expect("observer sink poisoned")
        .clone()
        .expect("验签通过却没把条目交给消费者——目录到了、模型清单仍是随包那份");
    assert_eq!(
        u64::from(version),
        observed_version,
        "状态报的版本与交给消费者的版本必须是同一份"
    );
    assert!(!entries.is_empty(), "已激活目录不该是空条目集");
    // 逐条解析成本版的条目形状：服务端发的每一条本版都该认得（跳过 = 两仓的条目定义漂了）。
    let (entries, skipped) = luna_model_catalog::parse_lenient(entries);
    assert_eq!(
        skipped, 0,
        "服务端目录里有本版认不出的条目——两仓条目定义不一致"
    );

    // 挑最小的一条真拉：判据是「拿得到字节」，不是「拿得到多少字节」，而每多下一个 GB
    // 就多一分让人把这条用例关掉的理由。
    let target = entries
        .iter()
        .min_by_key(|e| e.weights.size_bytes)
        .expect("上面已断言非空");
    println!(
        "拉取 {}（{} 字节，sha256 {}…）",
        target.id,
        target.weights.size_bytes,
        &target.weights.sha256[..12]
    );

    let artifacts = ArtifactClient::new(api_base.clone());
    let mirrors = artifacts
        .resolve_mirrors(&target.weights.sha256)
        .await
        .expect("制品面 resolve 应成功");
    assert!(
        !mirrors.is_empty(),
        "已激活目录里的 digest 必须解析得出镜像；空列表会让客户端每次都退回直连，\
         而那一条降级路径本身不报错"
    );

    // 未知 digest 必须 404（而不是回一个空镜像集）——两者在客户端的表现相同，
    // 但前者说得清「这个制品不在目录里」。
    let unknown = artifacts.resolve_mirrors(&"0".repeat(64)).await;
    assert!(
        unknown.is_err(),
        "不在 Active 目录里的 digest 必须被拒，实际返回 {unknown:?}"
    );

    let dest = std::env::temp_dir().join(format!(
        "lunarca-live-artifact-{}-{}",
        std::process::id(),
        target.id
    ));
    let _ = tokio::fs::remove_file(&dest).await;
    let progressed = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let ticks = progressed.clone();
    artifacts
        .fetch(
            &target.weights.urls(),
            &dest,
            target.weights.size_bytes,
            &target.weights.sha256,
            &move |bytes| {
                ticks.store(bytes, std::sync::atomic::Ordering::Relaxed);
            },
        )
        .await
        .unwrap_or_else(|e| panic!("制品下载失败：{e:?}"));

    let landed = tokio::fs::metadata(&dest)
        .await
        .expect("下载成功后文件必须在")
        .len();
    assert_eq!(
        landed, target.weights.size_bytes,
        "落盘字节数与目录声明不符——`fetch` 内部的大小判定被绕过了"
    );
    assert_eq!(
        progressed.load(std::sync::atomic::Ordering::Relaxed),
        target.weights.size_bytes,
        "进度回调的末值必须等于总字节数，否则界面上的进度条永远停在中途"
    );
    let _ = tokio::fs::remove_file(&dest).await;
    println!("✓ {} 字节已落盘并通过 sha256 终验", landed);
}

/// 提示词目录：**发布 → 客户端验签 → 真的装进注册表 → 防回滚水位拒绝更小版本**。
///
/// 这条补的是 ADR-050 唯一没被任何机器守护盯住的一段。离线用例证明状态机与签名正确，
/// 但证明不了「那份内容当真到得了 `PromptRegistry`」——而两者之间还隔着一道**完整性检查**：
/// 条目不齐时 `install_overlay` 会整份拒绝并沿用随包默认，此时管理端显示「已生效」、
/// 客户端用的仍是发版那天的模板，**两侧都没有任何东西会报错**。判据必须落在注册表上。
///
/// 服务端一侧的发布 / 回滚由运维脚本驱动（要运营密码）；本用例只验客户端那一半，
/// 并接受一个可选的内容标记来确认「回滚后拿到的确实是旧内容」。
///
/// ```sh
/// LUNA_CLOUD_API_BASE=https://10.0.0.62:8443 \
/// LUNA_CLOUD_CATALOG_PUBKEY=<64 hex> \
/// LUNA_CLOUD_PROMPT_MARKER=<回滚后应当出现的标记> \
///   cargo test -p luna-cloud --test service_live -- --ignored --nocapture prompt_catalog
/// ```
#[tokio::test]
#[ignore = "需要可达的 Lunarca 服务端与已激活的提示词目录"]
async fn prompt_catalog_reaches_the_registry_and_a_smaller_version_is_refused() {
    use luna_core::CapabilityPromptSet;

    let api_base = std::env::var("LUNA_CLOUD_API_BASE").expect("缺 LUNA_CLOUD_API_BASE");
    let api_base = api_base.trim_end_matches('/').to_owned();
    let pubkey = parse_pubkey_hex(
        &std::env::var("LUNA_CLOUD_CATALOG_PUBKEY").expect("缺 LUNA_CLOUD_CATALOG_PUBKEY"),
    )
    .expect("目录根公钥必须是 64 位十六进制");

    let seen: SeenPrompts = std::sync::Arc::new(std::sync::Mutex::new(None));
    let sink = seen.clone();
    let state_path =
        std::env::temp_dir().join(format!("lunarca-live-prompts-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&state_path);

    let prompts = CatalogClient::<CapabilityPromptSet>::new(
        CatalogKind::Prompts,
        api_base.clone(),
        Some(pubkey),
        std::sync::Arc::new(|| false),
    )
    .with_observer(std::sync::Arc::new(move |payload| {
        *sink.lock().expect("observer sink poisoned") = payload;
    }));
    prompts.attach_state_path(state_path.clone());

    let status = prompts.refresh().await.expect("提示词目录刷新请求应成功");
    assert_eq!(
        status.state,
        CatalogClientState::Verified,
        "提示词目录未验签通过（last_error={:?}）——`no_active_catalog` 说明服务端还没把\
         提示词发布走到 Active；`bad_keys` 说明钉的不是这台部署的根公钥，\
         `keys_expired` 说明该跑 `lunarcactl catalog-keys renew`",
        status.last_error
    );
    let version = status.verified_version.expect("已验签目录必须报得出版本号");
    let luna_cloud::CatalogDelivery {
        version: observed_version,
        variant,
        entries: sets,
    } = seen
        .lock()
        .expect("observer sink poisoned")
        .clone()
        .expect("验签通过却没把条目交给消费者——目录到了、模板仍是随包那份");
    assert_eq!(u64::from(version), observed_version);
    assert!(!sets.is_empty(), "已激活的提示词目录不该是空条目集");

    // **真判据在这里**：装得进注册表才叫「生效」。验签通过而条目不齐时它整份拒绝，
    // 而管理端那一侧显示的是「已生效」——两句话不可能同时为真，却都不会报错。
    let registry = luna_core::prompt::registry();
    registry
        .install_overlay(version, variant, sets.clone())
        .unwrap_or_else(|e| panic!("已验签的目录装不进注册表（条目不齐）：{e:?}"));
    println!(
        "✓ 提示词目录 v{version} 已验签并装进注册表（{} 个域）",
        sets.len()
    );

    if let Ok(marker) = std::env::var("LUNA_CLOUD_PROMPT_MARKER") {
        let hit = sets
            .iter()
            .flat_map(|s| s.templates.values())
            .any(|body| body.contains(&marker));
        assert!(
            hit,
            "服务端已回滚，客户端拿到的却不是那一版内容（找不到标记 {marker}）——\
             回滚在用户机器上等于什么都没发生"
        );
        println!("✓ 回滚后的内容标记 `{marker}` 已出现在客户端拿到的模板里");
    }
    registry.clear_overlay();

    // ---- 防回滚：水位高于服务端当前版本时，这一份必须被拒 ----
    //
    // 手法是把**水位**抬高，而不是让服务端发一个更小的版本——后者服务端根本做不到
    // （版本单调递增），而这正是 ADR-050 ④「回滚 = 旧内容重签更大版本」的由来：把
    // Active 指针挪回去的话，下发的版本更小，客户端会拒绝，回滚在用户机器上什么都不会发生。
    let bucket = format!("{}#prompts", luna_cloud::config::server_bucket(&api_base));
    let raised = serde_json::json!({
        bucket: {
            "last_verified_version": u64::from(version) + 1,
            // 载荷留空即可：水位在验签之前就把这一份判掉了。
            "payload": "",
            "signature": "",
            "verified_at": 0,
        }
    });
    std::fs::write(&state_path, raised.to_string()).expect("写高水位状态文件");

    let paranoid = CatalogClient::<CapabilityPromptSet>::new(
        CatalogKind::Prompts,
        api_base.clone(),
        Some(pubkey),
        std::sync::Arc::new(|| false),
    );
    paranoid.attach_state_path(state_path.clone());
    let status = paranoid.refresh().await.expect("刷新请求本身应成功");
    assert_eq!(
        status.state,
        CatalogClientState::Rejected,
        "水位是 v{} 而服务端下发 v{version}，必须被拒",
        u64::from(version) + 1
    );
    assert_eq!(
        status.last_error.as_deref(),
        Some("rollback"),
        "拒绝的原因必须是 rollback，而不是别的（那会指向另一类故障）"
    );
    println!("✓ 防回滚水位拒绝了更小的版本（last_error=rollback）");

    let _ = std::fs::remove_file(&state_path);
}

/// **强制 access token 过期之后的真实并发 401**（TDD-14 §3.3.1 ⑩ 的最后一项）。
///
/// 既有的并发回环用例打的是**假服务端**：它证明单飞逻辑本身对，但证不了「真的
/// 401 之后这条路走得通」——服务端拒的形状、refresh 家族的重放检测、以及轮换后新
/// access 立刻被复用，三件事都只有对着真服务端才成立。这条一直是 ⑩ 里最后一项未验。
///
/// 需要一枚**确实已经过期**的 access token 与其配套 refresh：
///
/// ```sh
/// LUNA_CLOUD_API_BASE=http://127.0.0.1:18090 \
/// LUNA_CLOUD_EXPIRED_ACCESS_TOKEN=... LUNA_CLOUD_REFRESH_TOKEN=... \
///   cargo test -p luna-cloud --test service_live -- --ignored --nocapture \
///     concurrent_401_after_a_real_expiry
/// ```
///
/// 判据四条：
///   ① 并发的四条控制面腿（设备 / 权益 / 用量 / 会话）**全部成功**，没有一条被踹回登录；
///   ② 本地 access token **确实换过**——没换就说明那枚 token 根本没过期，这条用例
///      什么也没验（比失败更糟：它会被读成通过）；
///   ③ 轮换之后会话仍然可用（再发一条照样成功）。若刚才发生了 refresh 重放，服务端会
///      吊销整个家族，这一条当场 `NeedsRelogin`；
///   ④ Provider 侧的入口（`refreshed_access_token`，两个 CloudProvider 与**批式** ASR
///      走的都是它；实时 ASR 只在建流时取一次 token，不走这条路）在同一轮里并发调用
///      同样不把会话打死。本用例守的是「不会重放、不会掉线」这条更硬的不变量——
///      「N 路并发只轮换一次」由离线用例
///      `client::session_scope_tests::provider_side_concurrent_refresh_never_replays_a_spent_token`
///      断言（那里数得到服务端侧的轮换次数，这里数不到）。
#[tokio::test]
#[ignore = "需要一枚已过期的 access token 与其配套 refresh"]
async fn concurrent_401_after_a_real_expiry_never_kills_the_session() {
    let api_base = std::env::var("LUNA_CLOUD_API_BASE").expect("缺 LUNA_CLOUD_API_BASE");
    let expired = std::env::var("LUNA_CLOUD_EXPIRED_ACCESS_TOKEN")
        .expect("缺 LUNA_CLOUD_EXPIRED_ACCESS_TOKEN");
    let refresh = std::env::var("LUNA_CLOUD_REFRESH_TOKEN").expect("缺 LUNA_CLOUD_REFRESH_TOKEN");
    let api_base = api_base.trim_end_matches('/').to_owned();

    let store = store_with_session(&api_base, &expired, &refresh);
    let client = std::sync::Arc::new({
        let client = CloudClient::new(Box::new(store), std::sync::Arc::new(|| false));
        client
            .account()
            .set_server(luna_cloud::account::AccountServer {
                origin: api_base.clone(),
                display_name: None,
            });
        client.reconfigure();
        client
    });

    // ① 四条控制面腿并发——账户页打开时真实发生的就是这一幕。
    let (devices, entitlements, usage, provider) = tokio::join!(
        client.devices(),
        client.entitlements(),
        client.usage(),
        // ④ Provider 侧入口挤在同一轮里：它与 `authed` 不是同一条路。
        // 递进去的是那枚**确已过期**的 access——真实 Provider 手上握着的正是它。
        {
            let c = client.clone();
            let rejected = expired.clone();
            async move { c.refreshed_access_token(&rejected).await }
        },
    );
    devices.expect("设备列表在过期后应静默续期并成功");
    entitlements.expect("权益在过期后应静默续期并成功");
    usage.expect("用量在过期后应静默续期并成功");
    assert!(
        provider.is_some(),
        "Provider 侧的续期入口在并发中拿不到新 access——线上表现是「用了一会儿突然要重新登录」"
    );

    // ② 真的换过。没换 = 那枚 token 没过期 = 这条用例什么都没验。
    let now_access = client.access_token().expect("会话还在");
    assert_ne!(
        now_access, expired,
        "access 没变——说明传进来的那枚并未过期，本用例没有验到任何东西"
    );

    // ③ 会话仍然可用：refresh 若被判重放，家族已被吊销，这里当场倒。
    client
        .entitlements()
        .await
        .expect("轮换之后会话必须仍然可用——失败即 refresh 被判重放、家族已吊销");
    println!("✓ 真实过期后的并发 401：四条腿全部成功，会话未被吊销");
}

/// **一次真实调用的用量，逐项对得上账户账本**（TDD-14 §3.3.1 ④/⑧ 的真机那一半）。
///
/// 服务端内部「终态帧 = 进账本的那一份」已有离线守护（商业仓
/// `inference_flow.rs::the_terminal_frame_carries_the_same_usage_that_goes_into_the_ledger`），
/// 客户端解两本账的 wire 也有单测。**没被任何东西盯住的是中间那道缝**：客户端
/// `GET /v1/usage` 读回来的那几个数，与这次调用实际产生的量是不是同一个数。对不上的
/// 表现是「界面写 137、账单写 152」，而谁都说不清哪个对——正是 ⑧ 拒绝显示估算值要
/// 消灭的那种假锚点。
///
/// 对话腿刻意用一段最小的 SSE 读取而不是 `CloudLlmProvider`：本 crate 不依赖
/// `luna-provider`（依赖方向如此），而**本用例要钉的是数字的对应关系**，请求形状与
/// 解码逻辑各自另有守护。
///
/// ```sh
/// LUNA_CLOUD_API_BASE=http://127.0.0.1:18090 \
/// LUNA_CLOUD_ACCESS_TOKEN=... LUNA_CLOUD_REFRESH_TOKEN=... \
///   cargo test -p luna-cloud --test service_live -- --ignored --nocapture usage_ledger
/// ```
#[tokio::test]
#[ignore = "需要可达的 Lunarca 服务端、有效会话与至少一个 chat 上游"]
async fn usage_ledger_matches_what_a_real_call_actually_consumed() {
    let api_base = std::env::var("LUNA_CLOUD_API_BASE").expect("缺 LUNA_CLOUD_API_BASE");
    let token = std::env::var("LUNA_CLOUD_ACCESS_TOKEN").expect("缺 LUNA_CLOUD_ACCESS_TOKEN");
    let refresh = std::env::var("LUNA_CLOUD_REFRESH_TOKEN").expect("缺 LUNA_CLOUD_REFRESH_TOKEN");
    let api_base = api_base.trim_end_matches('/').to_owned();

    let client = {
        let client = CloudClient::new(
            Box::new(store_with_session(&api_base, &token, &refresh)),
            std::sync::Arc::new(|| false),
        );
        client
            .account()
            .set_server(luna_cloud::account::AccountServer {
                origin: api_base.clone(),
                display_name: None,
            });
        client.reconfigure();
        client
    };

    /// 订阅那本账的逐维 `used`。
    fn subscription_used(
        ledgers: &[luna_cloud::client::UsageLedgerDto],
    ) -> std::collections::HashMap<String, f64> {
        let book = ledgers
            .iter()
            .find(|l| l.kind == "subscription")
            .expect("没有订阅账本——账户页会因此整块无从渲染");
        book.dimensions
            .iter()
            .map(|d| (d.dimension.clone(), d.used))
            .collect()
    }

    let before = client.usage().await.expect("读用量").ledgers;
    assert!(
        !before.iter().any(|l| l.kind == "prepaid"),
        "当前服务端没有预付账本的生产者——出现它说明多发了一本空账，\
         而界面会照着渲染出一段没有事实支撑的额度"
    );
    let before = subscription_used(&before);

    // ---- 一次真实对话 ----
    let http = reqwest::Client::new();
    let response = http
        .post(format!("{api_base}/inference/v1/jobs"))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "capability": "chat",
            // 协议 v5 起**必填**（ADR-083 ②）：缺它服务端回 400，且刻意没有 `serde(default)`。
            // 这条真机用例打的是对话，逐 token 上屏 ⇒ 实时档。
            "service_class": "realtime",
            "prompt_version": 0,
            "messages": [{"role": "user", "content": "用一句话说明什么是向量检索。"}],
        }))
        .send()
        .await
        .expect("建任务");
    assert!(
        response.status().is_success(),
        "建任务失败：{:?}",
        response.status()
    );
    let body = response.text().await.expect("读流");
    let mut usage: Option<serde_json::Value> = None;
    for line in body.lines() {
        let Some(payload) = line.strip_prefix("data:") else {
            continue;
        };
        let Ok(frame) = serde_json::from_str::<serde_json::Value>(payload.trim()) else {
            continue;
        };
        if frame.get("usage").is_some() {
            usage = frame.get("usage").cloned();
        }
    }
    let usage = usage.expect("终态帧必须带 usage（⑧ 恒发，`source` 一并带）");
    assert_eq!(
        usage["source"], "upstream",
        "`estimated` 说明我们没解出上游自报的用量——那一档客户端会显示「未知」，\
         而这里拿到的正是要与账本对齐的那个数：{usage}"
    );
    let tokens_in = usage["tokens_in"].as_f64().expect("tokens_in");
    let tokens_out = usage["tokens_out"].as_f64().expect("tokens_out");
    assert!(
        tokens_out > 0.0,
        "token 维恒为 0 就是 ⑧ 那条「从来没有过非零值」又回来了：{usage}"
    );

    // 账本是异步提交的（`MeterGuard::drop` 里 spawn 出去），给它一个有界的窗口。
    let mut after = None;
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let now = subscription_used(&client.usage().await.expect("读用量").ledgers);
        if now.get("requests").copied().unwrap_or_default()
            > before.get("requests").copied().unwrap_or_default()
        {
            after = Some(now);
            break;
        }
    }
    let after = after.expect("10 秒内账本没有记下这次调用——用量提交那一步断了");

    let delta = |dim: &str| {
        after.get(dim).copied().unwrap_or_default() - before.get(dim).copied().unwrap_or_default()
    };
    assert_eq!(
        delta("tokens_in"),
        tokens_in,
        "界面读到的 tokens_in 与这次调用实际消耗对不上"
    );
    assert_eq!(
        delta("tokens_out"),
        tokens_out,
        "界面读到的 tokens_out 与这次调用实际消耗对不上"
    );
    assert_eq!(delta("requests"), 1.0, "一次调用应当恰好记一条请求");
    assert!(
        delta("egress_bytes") > 0.0,
        "产出真的走了带宽，出站维不该纹丝不动"
    );
    println!("✓ 用量对账：tokens_in={tokens_in} tokens_out={tokens_out}，账本增量逐项一致");
}
