//! 指向库里内容的 `lunarca://` 链接（ADR-141，笔记库对标轮 W4a）：解析与生成，纯逻辑。
//!
//! - `lunarca://open?vault=<库 id>&path=<库相对路径>[#标题]` —— 打开一篇（可定位到标题）；
//! - `lunarca://search?vault=<库 id>&q=<查询>` —— 在搜索里搜；
//! - `lunarca://new?vault=<库 id>&name=<名字>` —— 打开**预填**的新建卡，用户确认后才写盘。
//!
//! 安全边界在这里先过一道：`vault` 只能是库 id（不接受文件系统路径，查注册表在调用方）；
//! `path` 走 [`crate::pathutil::external_rel_path`]——外部链接只能指向用户的笔记与附件。
//! 链接里的内容不写日志（ADR-141 ⑥）。
//!
//! 放在 `luna-vault` 而不是应用层：CLI 的写意图（`luna note new`）也是生成这里的链接交给应用
//! 确认（ADR-141 ④），链接格式只有这一份。

use url::Url;

const SCHEME: &str = "lunarca";

/// 链接要做的事。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VaultLinkAction {
    Open {
        path: String,
        heading: Option<String>,
    },
    Search {
        query: String,
    },
    New {
        name: String,
    },
}

/// 一条解析好的链接。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultLink {
    /// 库 id（`.luna/vault-id`，ADR-140 ②）。
    pub vault: String,
    pub action: VaultLinkAction,
}

/// 链接不成立的理由（界面按它说人话；不带链接内容）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultLinkRejection {
    /// 不是这三种链接之一，或缺了必需的参数。
    Invalid,
    /// `path` 越出库、是绝对路径或指向隐藏 / Lunarca 自己的数据。
    BadPath,
}

/// 这个 host 是不是本模块管的链接（入口据此分派，ADR-141 ⑥）。
#[must_use]
pub fn is_vault_link(input: &str) -> bool {
    Url::parse(input.trim()).is_ok_and(|url| {
        url.scheme() == SCHEME && matches!(url.host_str(), Some("open" | "search" | "new"))
    })
}

/// 解析一条链接。
///
/// # Errors
/// 不是本模块的链接、缺参数、或路径不合法。
pub fn parse(input: &str) -> Result<VaultLink, VaultLinkRejection> {
    let url = Url::parse(input.trim()).map_err(|_| VaultLinkRejection::Invalid)?;
    if url.scheme() != SCHEME {
        return Err(VaultLinkRejection::Invalid);
    }
    let param = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    let vault = param("vault").ok_or(VaultLinkRejection::Invalid)?;
    let action = match url.host_str() {
        Some("open") => {
            let path = param("path").ok_or(VaultLinkRejection::Invalid)?;
            VaultLinkAction::Open {
                path: crate::pathutil::external_rel_path(&path)
                    .ok_or(VaultLinkRejection::BadPath)?,
                heading: url
                    .fragment()
                    .map(|fragment| {
                        luna_sync_core::links::percent_decode(fragment)
                            .trim()
                            .to_owned()
                    })
                    .filter(|heading| !heading.is_empty()),
            }
        }
        Some("search") => VaultLinkAction::Search {
            query: param("q").ok_or(VaultLinkRejection::Invalid)?,
        },
        Some("new") => {
            let name = param("name").ok_or(VaultLinkRejection::Invalid)?;
            if name.contains(['/', '\\']) {
                return Err(VaultLinkRejection::Invalid);
            }
            VaultLinkAction::New { name }
        }
        _ => return Err(VaultLinkRejection::Invalid),
    };
    Ok(VaultLink { vault, action })
}

/// 一篇笔记的 `open` 链接（右键「复制 Lunarca 链接」）。参数按 URL 规则编码。
#[must_use]
pub fn open_link(vault: &str, path: &str) -> String {
    let mut url = Url::parse("lunarca://open").expect("固定前缀合法");
    url.query_pairs_mut()
        .append_pair("vault", vault)
        .append_pair("path", path);
    url.to_string()
}

/// 新建一篇的 `new` 链接（`luna note new`：CLI 不写库，交给应用、由用户确认后才落盘）。
#[must_use]
pub fn new_link(vault: &str, name: &str) -> String {
    let mut url = Url::parse("lunarca://new").expect("固定前缀合法");
    url.query_pairs_mut()
        .append_pair("vault", vault)
        .append_pair("name", name);
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "01a10c9a-670b-72d1-a3cb-675742d38201";

    #[test]
    fn parses_the_three_actions() {
        assert_eq!(
            parse(&format!("lunarca://open?vault={ID}&path=%E9%A1%B9%E7%9B%AE/%E8%AE%A1%E5%88%92.md#%E8%AE%AE%E7%A8%8B")).unwrap(),
            VaultLink {
                vault: ID.into(),
                action: VaultLinkAction::Open {
                    path: "项目/计划.md".into(),
                    heading: Some("议程".into()),
                },
            }
        );
        assert_eq!(
            parse(&format!(
                "lunarca://search?vault={ID}&q=%E5%AD%A3%E5%BA%A6+tag:%E5%B7%A5%E4%BD%9C"
            ))
            .unwrap()
            .action,
            VaultLinkAction::Search {
                query: "季度 tag:工作".into()
            }
        );
        assert_eq!(
            parse(&format!("lunarca://new?vault={ID}&name=%E5%91%A8%E4%BC%9A"))
                .unwrap()
                .action,
            VaultLinkAction::New {
                name: "周会".into()
            }
        );
    }

    #[test]
    fn paths_must_stay_inside_and_away_from_hidden_data() {
        for bad in [
            "../x.md",
            "a/../../x.md",
            "/etc/passwd",
            ".luna/index.sqlite",
            "a/.trash/x.md",
            "C:%5Cx.md",
            ".obsidian/app.json",
        ] {
            assert_eq!(
                parse(&format!("lunarca://open?vault={ID}&path={bad}")),
                Err(VaultLinkRejection::BadPath),
                "{bad}"
            );
        }
    }

    #[test]
    fn missing_params_and_other_hosts_are_invalid() {
        assert_eq!(
            parse("lunarca://open?path=a.md"),
            Err(VaultLinkRejection::Invalid)
        );
        assert_eq!(
            parse(&format!("lunarca://open?vault={ID}")),
            Err(VaultLinkRejection::Invalid)
        );
        assert_eq!(
            parse(&format!("lunarca://new?vault={ID}&name=a/b")),
            Err(VaultLinkRejection::Invalid)
        );
        assert_eq!(
            parse(&format!("lunarca://delete?vault={ID}")),
            Err(VaultLinkRejection::Invalid)
        );
        assert!(!is_vault_link("lunarca://connect?v=1"));
        assert!(is_vault_link(&format!("lunarca://search?vault={ID}&q=a")));
    }

    #[test]
    fn generated_links_round_trip() {
        let link = open_link(ID, "项目/周会 纪要&附录.md");
        assert_eq!(
            parse(&link).unwrap().action,
            VaultLinkAction::Open {
                path: "项目/周会 纪要&附录.md".into(),
                heading: None,
            }
        );
        assert_eq!(
            parse(&new_link(ID, "周会 & 复盘")).unwrap().action,
            VaultLinkAction::New {
                name: "周会 & 复盘".into()
            }
        );
    }
}
