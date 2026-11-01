//! 「**检查与读取绑定在同一个对象上**」的安全打开原语（TDD-07 §2.1 输入快照 / ADR-025）。
//!
//! **这里修的是一类结构性错误，不是某一处笔误**：先按路径校验、再按同一个路径重新打开
//! 读取，是两次**互相独立**的文件系统解析。两次之间只要有人换掉路径上任意一段（一条软链、
//! 一层父目录），校验看到的对象与读到的对象就不是同一个——而汇总会把读到的正文原样喂给
//! 模型并写进产物，那是一条把库外内容搬进库里的通路。这条通路在自动化全绿时完全不可见：
//! 单线程跑测试永远不会在那两次解析之间插进一次替换。
//!
//! 原语给出的顺序是**唯一**的：
//! 1. `File::open` —— 先真的把它打开，后面一切判定都对着**这一个句柄**说话；
//! 2. `Handle::from_file` —— 身份取自句柄本身；
//! 3. `canonicalize` —— 这条路径此刻真实解析到哪儿，是不是还在库根内；
//! 4. **句柄身份 == 已验证路径此刻指向的对象** —— 这一步才是把 ① 与 ③ 焊在一起的那一焊；
//! 5. 从**同一个句柄**读内容，不按路径重开。
//!
//! 少了第 4 步，前三步各自都对、合起来仍然是错的。

use std::fs::File;

/// Vault 内一个文件**对象**的身份——由已打开的句柄取得，**不是路径字符串**。
///
/// **为什么身份不能是路径**：库内一条软链、以及大小写不敏感卷（APFS 默认）上的
/// `Note.md` / `note.md`，都是同一个对象的不同写法。按字符串去重会把同一篇算成两条：
/// 条数、来源区、以及"至少选 2 条"这条规则同时失真，而界面上毫无痕迹。
///
/// 反过来也**刻意不做大小写折叠**：折叠是把平台差异搬进业务层，在大小写敏感卷上会把两个
/// 真正不同的文件误判成一个。判据只有一个——操作系统认为它们是不是同一个对象。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ObjectId(Repr);

// Unix：`(dev, ino)` 是稳定的**数值**身份，取完即可放掉文件描述符。
//
// **必须放掉**：一次汇总最多 200 篇源，而 macOS 进程的默认 fd 软上限是 256。攥着 200 个
// 描述符跨过分钟级的模型等待，会把整个进程推到"打不开任何文件"的边上——那是一次汇总把
// 整个应用拖垮，而症状会出现在与汇总毫无关系的地方。
#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Repr {
    dev: u64,
    ino: u64,
}

// Windows：卷序号 + 文件序号只有 `GetFileInformationByHandle` 给得出，而 `same_file::Handle`
// 只暴露**比较**、不暴露那两个数（标准库的出口至今是 nightly）。故这里留着句柄本身当身份。
// Windows 的句柄配额是百万量级，不存在 Unix 那条 fd 上限的顾虑；`File::open` 的共享模式
// 也允许别人继续改名/删除，攥着它不会挡住用户。
#[cfg(not(unix))]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Repr(std::sync::Arc<same_file::Handle>);

impl ObjectId {
    /// 从一个**已打开的句柄**取身份。
    #[cfg(unix)]
    fn from_handle(handle: same_file::Handle) -> Self {
        Self(Repr {
            dev: handle.dev(),
            ino: handle.ino(),
        })
    }

    /// 从一个**已打开的句柄**取身份。
    #[cfg(not(unix))]
    fn from_handle(handle: same_file::Handle) -> Self {
        Self(Repr(std::sync::Arc::new(handle)))
    }
}

/// [`crate::Vault::open_in_vault`] 一次安全打开的结果。
#[derive(Clone, Debug)]
pub struct VaultFile {
    /// 归一后的 Vault 相对路径（调用方传进来的那一条，只做斜杠归一）。
    pub rel: String,
    /// 对象身份（取自句柄，见 [`ObjectId`]）。
    pub id: ObjectId,
    /// 从**同一个句柄**读出的完整内容。
    pub content: String,
}

/// 安全打开被挡住的原因。
///
/// 刻意只有两类：调用方要么把它翻成自己那套错误码，要么整体拒绝——中间没有"读到一半"。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultOpenError {
    /// 越出 Vault 根：`..` / 绝对路径 / 盘符，或经符号链接指到库外，
    /// 或**句柄与已验证路径对不上**（检查期间路径被换掉了）。
    PathEscape,
    /// 打不开 / 读不到 / 不是文本（不存在、已被移走、权限、IO 失败）。
    Unreadable,
}

/// 打开 `abs` 并把「身份 / 库根校验 / 读取」绑在同一个对象上。
///
/// `root` 为 Vault 根的绝对路径。`abs` 必须已过词法防护（`..` / 绝对路径由调用方先挡）。
pub(crate) fn open_bound(
    abs: &std::path::Path,
    root: &std::path::Path,
) -> std::result::Result<(ObjectId, String), VaultOpenError> {
    open_bound_with_limit(abs, root, None)
}

/// 与安全打开同一条检查链；大小限制在已验证的同一个句柄上执行。
pub(crate) fn open_bound_limited(
    abs: &std::path::Path,
    root: &std::path::Path,
    max_bytes: usize,
) -> std::result::Result<(ObjectId, String), VaultOpenError> {
    open_bound_with_limit(abs, root, Some(max_bytes))
}

fn open_bound_with_limit(
    abs: &std::path::Path,
    root: &std::path::Path,
    max_bytes: Option<usize>,
) -> std::result::Result<(ObjectId, String), VaultOpenError> {
    use std::io::Read;
    use VaultOpenError as E;

    // ① 先打开。后面每一步说的"这个文件"都是指这个句柄，不是那条路径。
    let file = File::open(abs).map_err(|_| E::Unreadable)?;
    // ② 身份必须走 `from_file`，**绝不是** `from_path`：后者会按路径再解析一次，
    //    等于在"检查"与"读取"之间亲手又开一个可被替换的窗口——正是本原语要关掉的那个。
    let mut handle = same_file::Handle::from_file(file).map_err(|_| E::Unreadable)?;
    // ③ 真实路径必须落在库根内。词法检查看不见软链：`外链.md` 字面上完完全全是一条库内
    //    相对路径，解析到的却可以是库外的文件。
    let (real, root) = match (std::fs::canonicalize(abs), std::fs::canonicalize(root)) {
        (Ok(real), Ok(root)) => (real, root),
        _ => return Err(E::Unreadable),
    };
    if !real.starts_with(&root) {
        return Err(E::PathEscape);
    }
    // ④ **把 ② 与 ③ 焊在一起**：我手里这个句柄，必须就是那条**已验证路径**此刻指向的
    //    对象。这里用 `from_path` 是对的、也是唯一的用法——它开的是"已经验证过在库内的
    //    那条真实路径"，而不是待判定的输入；比较的结果只有两种：要么两边是同一个对象
    //    （于是 ③ 的结论对 ② 成立），要么期间被换过（当场拒绝，fail closed）。
    //    删掉这一步，①③⑤ 就退回成三次互不相干的解析——那正是修复前的形状。
    let verified = same_file::Handle::from_path(&real).map_err(|_| E::Unreadable)?;
    if handle != verified {
        return Err(E::PathEscape);
    }
    // ⑤ 从同一个句柄读，不按路径重开。
    let mut content = String::new();
    if let Some(limit) = max_bytes {
        if handle
            .as_file()
            .metadata()
            .map_err(|_| E::Unreadable)?
            .len()
            > limit as u64
        {
            return Err(E::Unreadable);
        }
        handle
            .as_file_mut()
            .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
            .read_to_string(&mut content)
            .map_err(|_| E::Unreadable)?;
        if content.len() > limit {
            return Err(E::Unreadable);
        }
    } else {
        handle
            .as_file_mut()
            .read_to_string(&mut content)
            .map_err(|_| E::Unreadable)?;
    }
    Ok((ObjectId::from_handle(handle), content))
}

#[cfg(test)]
mod tests {
    use crate::{ObjectId, Vault, VaultError, VaultOpenError};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    fn open_vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = Vault::open(dir.path()).expect("open vault");
        (dir, vault)
    }

    fn note(body: &str) -> String {
        format!("---\nluna:\n  type: note\n---\n\n{body}\n")
    }

    /// **本批修的那条通路的直接反证**：检查通过之后、读取之前把软链换掉，读到的必须
    /// 永远不是库外的字节。
    ///
    /// 修复前的形状是「按路径校验一次 → 按同一路径**重新打开**读一次」，两次解析之间只要
    /// 有一次替换落进去，库外内容就会被原样喂给模型并写进产物。单线程跑测试永远不会在那
    /// 两次解析之间插进一次替换——所以这条只能用真实交错来证。
    ///
    /// 断言的是**不变量**（成功的那些次里一个库外字节都没有），不是某一次具体的调度：
    /// 调度不可复现，不变量必须永远成立。`ok > 0` 只是确认这一轮真的读到过东西、不是一条
    /// 恒空跑的守护。
    #[cfg(unix)]
    #[test]
    fn a_symlink_swapped_mid_check_never_yields_bytes_from_outside_the_vault() {
        const ROUNDS: usize = 2000;
        let (_dir, vault) = open_vault();
        let outside = tempfile::tempdir().expect("outside");
        let secret = outside.path().join("库外.md");
        std::fs::write(&secret, note("OUTSIDE-SECRET")).expect("写库外文件");
        vault.write("库内.md", &note("INSIDE")).expect("写库内文件");

        let root = vault.root().to_path_buf();
        let alias = root.join("别名.md");
        let inside = root.join("库内.md");
        let gate = Arc::new(Barrier::new(2));
        let swapper = {
            let (gate, alias, inside, secret) =
                (Arc::clone(&gate), alias.clone(), inside, secret.clone());
            std::thread::spawn(move || {
                for round in 0..ROUNDS {
                    let target = if round % 2 == 0 { &inside } else { &secret };
                    // 换链必须原子：`symlink` 撞到既有条目会失败，故建在临时名上再 rename。
                    let staging = alias.with_extension(format!("swap{round}"));
                    std::os::unix::fs::symlink(target, &staging).expect("建软链");
                    gate.wait();
                    std::fs::rename(&staging, &alias).expect("换链");
                }
            })
        };

        let mut ok = 0usize;
        let mut rejected = 0usize;
        for _ in 0..ROUNDS {
            gate.wait();
            match vault.merge_source("别名.md") {
                Ok(source) => {
                    assert!(
                        !source.input.contains("OUTSIDE-SECRET"),
                        "库外字节经软链进了汇总输入"
                    );
                    ok += 1;
                }
                Err(_) => rejected += 1,
            }
        }
        swapper.join().expect("换链线程");
        assert_eq!(ok + rejected, ROUNDS);
        assert!(ok > 0, "这一轮一次都没读成功，守护等于空跑");
    }

    /// Vault 根本身也必须在打开时锚定。只在每次读文件时 canonicalize 当前根，会让一条被
    /// 替换的根软链把整个 Vault 悄悄改指向库外，而单篇文件的句柄检查仍会“自洽”通过。
    #[cfg(unix)]
    #[test]
    fn replacing_the_root_symlink_cannot_redirect_an_open_vault() {
        let parent = tempfile::tempdir().expect("parent");
        let inside = parent.path().join("inside");
        let outside = parent.path().join("outside");
        std::fs::create_dir_all(&inside).expect("inside");
        std::fs::create_dir_all(&outside).expect("outside");
        std::fs::write(inside.join("同名.md"), note("INSIDE")).expect("inside note");
        std::fs::write(outside.join("同名.md"), note("OUTSIDE-SECRET")).expect("outside note");

        let alias = parent.path().join("vault");
        std::os::unix::fs::symlink(&inside, &alias).expect("link inside");
        let vault = Vault::open(&alias).expect("open through alias");
        assert_eq!(vault.root(), std::fs::canonicalize(&inside).unwrap());

        std::fs::remove_file(&alias).expect("remove alias");
        std::os::unix::fs::symlink(&outside, &alias).expect("redirect alias");
        let source = vault
            .merge_source("同名.md")
            .expect("still reads anchored vault");
        assert!(source.input.contains("INSIDE"));
        assert!(!source.input.contains("OUTSIDE-SECRET"));
    }

    /// 库内的一条软链别名与它指向的那一篇是**同一个对象**——身份取自句柄，不取自路径。
    #[cfg(unix)]
    #[test]
    fn an_in_vault_alias_and_its_target_share_one_object_identity() {
        let (_dir, vault) = open_vault();
        vault.write("真身.md", &note("同一段正文")).expect("写笔记");
        std::os::unix::fs::symlink(vault.root().join("真身.md"), vault.root().join("别名.md"))
            .expect("建库内软链");

        let real = vault.open_in_vault("真身.md").expect("真身");
        let alias = vault.open_in_vault("别名.md").expect("别名");
        assert_eq!(real.id, alias.id, "别名与真身必须是同一个对象");
        assert_eq!(real.content, alias.content);
        // 相对路径仍如实保留调用方给的那一条（去重按身份，不按它）。
        assert_eq!(alias.rel, "别名.md");
    }

    /// 大小写变体：在大小写不敏感卷（macOS APFS 默认）上是同一个对象；在敏感卷上是两个。
    ///
    /// **刻意不做大小写折叠**：折叠是把平台差异搬进业务层，在敏感卷上会把两个真正不同
    /// 的文件误判成一个。故本条按**操作系统怎么看**断言，不按字符串怎么看。
    #[test]
    fn a_case_variant_is_one_object_exactly_when_the_filesystem_says_so() {
        let (_dir, vault) = open_vault();
        vault.write("Note.md", &note("正文")).expect("写笔记");
        let exact = vault.open_in_vault("Note.md").expect("原名");
        match vault.open_in_vault("note.md") {
            // 大小写不敏感卷：两条路径打开的是同一个对象。
            Ok(variant) => assert_eq!(exact.id, variant.id, "同一个 inode 必须是同一个身份"),
            // 大小写敏感卷：`note.md` 根本不存在，这也是对的。
            Err(error) => assert_eq!(error, VaultOpenError::Unreadable),
        }
    }

    /// 两个真正不同的对象，身份必须不同（去重不能把不同来源折成一条）。
    #[test]
    fn two_distinct_files_never_share_an_identity() {
        let (_dir, vault) = open_vault();
        vault.write("甲.md", &note("同一段正文")).expect("甲");
        vault.write("乙.md", &note("同一段正文")).expect("乙");
        let a = vault.open_in_vault("甲.md").expect("甲");
        let b = vault.open_in_vault("乙.md").expect("乙");
        assert_eq!(a.content, b.content, "内容刻意一模一样");
        assert_ne!(a.id, b.id, "内容相同不代表是同一个对象");
    }

    #[test]
    fn the_primitive_rejects_everything_that_leaves_the_vault() {
        let (_dir, vault) = open_vault();
        for escape in ["../外面.md", "/etc/passwd", "a/../../外面.md"] {
            assert_eq!(
                vault.open_in_vault(escape).unwrap_err(),
                VaultOpenError::PathEscape,
                "{escape}"
            );
        }
        assert_eq!(
            vault.open_in_vault("不存在.md").unwrap_err(),
            VaultOpenError::Unreadable
        );
    }

    /// **`link(2)` 的 `EEXIST` 是唯一的"落点被占"判据**：两条路同时挑中同一个落点时，
    /// 恰好一条成功、另一条整体放弃——**不是**换个名字写下去（换名会让 durable intent
    /// 指着另一篇笔记，知识生命周期随后作用在错的对象上）。
    #[test]
    fn two_concurrent_publishes_to_one_target_leave_exactly_one_winner() {
        for _ in 0..20 {
            let dir = tempfile::tempdir().expect("tempdir");
            // `Vault` 不是 `Sync`（内含 rusqlite `Connection`），而两条真实的并发写入路本
            // 来就各持一个句柄——故两个线程各开一个 Vault，这比共享一个更接近现场。
            let winners = Arc::new(AtomicUsize::new(0));
            let taken = Arc::new(AtomicUsize::new(0));
            let gate = Arc::new(Barrier::new(2));
            let hands: Vec<_> = ["甲", "乙"]
                .into_iter()
                .map(|who| {
                    let vault = Vault::open(dir.path()).expect("open vault");
                    let (gate, winners, taken) =
                        (Arc::clone(&gate), Arc::clone(&winners), Arc::clone(&taken));
                    std::thread::spawn(move || {
                        gate.wait();
                        match vault.publish_new("Notes/汇总.md", &note(who)) {
                            Ok(()) => winners.fetch_add(1, Ordering::Relaxed),
                            Err(VaultError::AlreadyExists(_)) => {
                                taken.fetch_add(1, Ordering::Relaxed)
                            }
                            Err(other) => panic!("既不是成功也不是被占：{other:?}"),
                        };
                    })
                })
                .collect();
            for hand in hands {
                hand.join().expect("发布线程");
            }
            assert_eq!(winners.load(Ordering::Relaxed), 1, "落点被写了不止一次");
            assert_eq!(taken.load(Ordering::Relaxed), 1, "输的那一条没有整体放弃");
            // 落地的是完整内容，不是零字节占位。
            let on_disk = std::fs::read_to_string(dir.path().join("Notes/汇总.md")).expect("读回");
            assert!(
                on_disk.contains("甲") || on_disk.contains("乙"),
                "{on_disk}"
            );
            // 临时文件不留在用户 Vault 里。
            let leftovers = std::fs::read_dir(dir.path().join("Notes"))
                .expect("列目录")
                .filter_map(std::result::Result::ok)
                .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
                .count();
            assert_eq!(leftovers, 0, "临时文件没清干净");
        }
    }

    /// **读不出来的既有笔记也是"占着的"**：修复前的判据是 `read(target).is_ok()`
    /// ——读失败（权限 / 瞬时 IO）会被当成"目标不存在"，下一步就写在用户已有的资产上。
    #[cfg(unix)]
    #[test]
    fn an_unreadable_existing_note_is_never_overwritten() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, vault) = open_vault();
        let target = vault.root().join("占用.md");
        std::fs::write(&target, note("用户的资产")).expect("写既有笔记");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o000)).expect("改权限");
        if std::fs::read_to_string(&target).is_ok() {
            // root 身份跑测试时权限不生效——如实跳过，不假装验过。
            return;
        }

        // ① 唯一命名不会挑中它（判据是"文件系统里有没有这个条目"，不是"读不读得出来"）。
        assert!(!vault.path_is_free("占用.md"), "读不出来不等于是空位");
        assert!(vault.path_is_free("占用-1.md"));
        // ② 即使调用方硬挑了它，发布也只会被内核挡回，绝不覆盖。
        assert!(matches!(
            vault.publish_new("占用.md", &note("汇总产物")),
            Err(VaultError::AlreadyExists(_))
        ));

        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).expect("复原");
        assert!(
            std::fs::read_to_string(&target)
                .expect("读回")
                .contains("用户的资产"),
            "用户的笔记被动过了"
        );
    }

    /// 悬空软链同样占着这个名字（`metadata` 会跟随它并报"不存在"，`symlink_metadata` 不会）。
    #[cfg(unix)]
    #[test]
    fn a_dangling_symlink_still_occupies_the_name() {
        let (_dir, vault) = open_vault();
        std::os::unix::fs::symlink(
            vault.root().join("从来没有过.md"),
            vault.root().join("悬空.md"),
        )
        .expect("建悬空软链");
        assert!(!vault.path_is_free("悬空.md"));
        assert!(matches!(
            vault.publish_new("悬空.md", &note("汇总产物")),
            Err(VaultError::AlreadyExists(_))
        ));
    }

    /// 发布成功 = 这一篇是**刚刚被创建**的（知识度量的"新建"由此成为事实而非回读推断）。
    #[test]
    fn publishing_a_new_note_really_creates_it() {
        let (_dir, vault) = open_vault();
        assert!(vault.path_is_free("Notes/汇总.md"));
        vault
            .publish_new("Notes/汇总.md", &note("汇总产物"))
            .expect("发布");
        assert!(!vault.path_is_free("Notes/汇总.md"));
        assert!(vault
            .read("Notes/汇总.md")
            .expect("读回")
            .content
            .contains("汇总产物"));
        // 派生索引跟上（与 `Vault::write` 同一口径，否则新笔记在搜索里不存在）。
        assert!(vault
            .search(&crate::Query {
                limit: 50,
                ..crate::Query::default()
            })
            .expect("查询")
            .iter()
            .any(|hit| hit.path == "Notes/汇总.md"));
        // 第二次落到同一个名字必须被挡回。
        assert!(matches!(
            vault.publish_new("Notes/汇总.md", &note("另一份")),
            Err(VaultError::AlreadyExists(_))
        ));
    }

    /// 跨模型等待之后的逐条复验：改稿 / 沉淀 / 移走，任一项都必须被察觉。
    #[test]
    fn the_publish_time_recheck_notices_every_way_a_source_can_change() {
        let (_dir, vault) = open_vault();
        vault.write("甲.md", &note("甲的正文")).expect("甲");
        vault
            .write(
                "乙.md",
                "---\nluna:\n  type: note\n  stage: draft\n---\n\n乙的正文\n",
            )
            .expect("乙");
        let snapshot = vec![
            vault.merge_source("甲.md").expect("甲"),
            vault.merge_source("乙.md").expect("乙"),
        ];
        assert!(vault.merge_sources_unchanged(&snapshot), "刚取完就该一致");

        // ① 正文被改。
        vault.write("甲.md", &note("改过了")).expect("改甲");
        assert!(!vault.merge_sources_unchanged(&snapshot));
        // 改回同样的字即算没变过——判据是**内容**，不是"这个文件被写过没有"。
        // （原子写换掉了 inode，故这一条同时钉住"身份不参与复验"这个裁决。）
        vault.write("甲.md", &note("甲的正文")).expect("改回");
        assert!(vault.merge_sources_unchanged(&snapshot), "改回来即一致");

        // ② 被沉淀掉——正文一个字节没动，但它已经不该出现在这次汇总里。
        vault.promote_to_knowledge("乙.md").expect("沉淀");
        assert!(!vault.merge_sources_unchanged(&snapshot));

        // ③ 被移走 / 删掉：读不到即视为变了，整体放弃。
        vault.write("甲.md", &note("甲的正文")).expect("复原甲");
        std::fs::remove_file(vault.root().join("乙.md")).expect("删乙");
        assert!(!vault.merge_sources_unchanged(&snapshot));
    }

    /// **这条守护解释的是"为什么光有指纹复验不够"**：另一个库里的同名同内容文件，
    /// 指纹逐字节相等——复验会当场放行。
    ///
    /// 所以「切库」不能靠源复验兜，只能靠**库代际**（ADR-025），而代际必须与 sources 在
    /// **同一个临界区**里取：晚一步取到的是新库的代际，与旧库的 sources 一比恒相等，
    /// 判别力当场归零。本条钉住这个前提，代际本身的取用点由 `distill.rs` 的结构守护锁住。
    #[test]
    fn a_fingerprint_recheck_alone_cannot_tell_two_vaults_apart() {
        let (_dir_a, vault_a) = open_vault();
        let (_dir_b, vault_b) = open_vault();
        vault_a.write("甲.md", &note("一模一样的正文")).expect("甲");
        vault_b
            .write("甲.md", &note("一模一样的正文"))
            .expect("另一个库里的同名文件");
        let snapshot = vec![vault_a.merge_source("甲.md").expect("快照")];
        assert!(vault_a.merge_sources_unchanged(&snapshot));
        assert!(
            vault_b.merge_sources_unchanged(&snapshot),
            "指纹本就分不出两个库——分得出的话，代际检查就成了多余的，而它不是"
        );
        // 而**对象身份**分得出来：它是"这两条是不是同一篇"的答案，只是不能拿去当"有没有
        // 变过"的答案（原子写会换 inode，见 `Vault::merge_sources_unchanged` 的说明）。
        assert_ne!(
            snapshot[0].id,
            vault_b.merge_source("甲.md").expect("另一个库").id
        );
    }

    #[test]
    fn identities_are_usable_as_set_keys() {
        let (_dir, vault) = open_vault();
        vault.write("甲.md", &note("正文")).expect("甲");
        let a = vault.open_in_vault("甲.md").expect("甲").id;
        let b = vault.open_in_vault("甲.md").expect("再开一次").id;
        let set: std::collections::HashSet<ObjectId> = [a.clone(), b].into_iter().collect();
        assert_eq!(set.len(), 1, "同一个对象两次打开必须落进同一个桶");
    }

    #[test]
    fn limited_safe_open_rejects_oversized_content_and_keeps_object_identity() {
        let (_dir, vault) = open_vault();
        vault.write("甲.md", "12345").unwrap();
        assert_eq!(
            vault.open_in_vault_limited("甲.md", 4).unwrap_err(),
            VaultOpenError::Unreadable
        );
        let limited = vault.open_in_vault_limited("甲.md", 5).unwrap();
        let ordinary = vault.open_in_vault("甲.md").unwrap();
        assert_eq!(limited.content, ordinary.content);
        assert_eq!(limited.id, ordinary.id);
    }
}
