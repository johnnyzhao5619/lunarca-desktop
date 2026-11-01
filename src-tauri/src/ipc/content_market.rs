//! 在线内容目录的共用那一段（纵切 7，ADR-159 ③ / ADR-168 ⑦）：主题与模板两类目录「此刻能不能用」与「检查更新」。
//!
//! 两类各自的条目、状态与安装在 `themes` / `template_market`；这里只放两边一模一样的判定，免得一边改了另一边没跟上
//! （2026-10-23 Windows 真机：「已安装」里的「检查更新」曾跟着「获取更多主题」那一块一起消失）。

use super::*;

/// 在线内容目录此刻能不能用（已登录、未开严格本地、云插件已装上）——「检查更新」与「获取更多」的共同前提；
/// 某一份目录里有没有可列的条目另说。
pub(crate) fn online_catalog_usable(app: &tauri::AppHandle) -> bool {
    crate::cloud::evaluate_for(app).allowed()
        && app.try_state::<luna_cloud::commands::Catalogs>().is_some()
}

/// 「检查更新」问的是哪一份目录。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ContentCatalogDto {
    Themes,
    Templates,
}

/// 一份目录这次刷新的结果 → 「查没查成」（**纯函数**）。刷新失败在目录状态机里**不是 Err**（进 `Rejected`、沿用缓存，
/// 见 `CatalogClient::refresh`），所以要按原因码判：服务端没有生效的那份目录（`no_active_catalog`）也算查过了——已缓存
/// 的那一版照用到过期，没有更新可言；其余原因（连不上、验签不过、过期……）都是没查成。
fn check_outcome(catalog: ContentCatalogDto, last_error: Option<&str>) -> Result<(), String> {
    match last_error {
        None | Some("no_active_catalog") => Ok(()),
        Some(reason) => Err(format!(
            "CONTENT_CHECK_FAILED: {}:{reason}",
            match catalog {
                ContentCatalogDto::Themes => "themes",
                ContentCatalogDto::Templates => "templates",
            }
        )),
    }
}

/// 打开「应用」或「设置 › 本地模型」时的重验（ADR-170 ②）：门控放行且距上次刷新超过 5 分钟才在这里拉一遍全部
/// 目录；界面先用缓存渲染，交付一变由 `themes://changed` / `templates://changed` 与调用方重读带出。门控不放行
/// （未登录、严格本地）时一个字节都不发（ADR-037），也不报错——这不是用户点的动作。
#[tauri::command]
#[specta::specta]
pub(crate) async fn revalidate_catalogs(app: tauri::AppHandle) {
    if !online_catalog_usable(&app) {
        return;
    }
    let Some(catalogs) = app
        .try_state::<luna_cloud::commands::Catalogs>()
        .map(|catalogs| catalogs.inner().clone())
    else {
        return;
    };
    catalogs
        .refresh_if_stale(luna_cloud::catalog::ON_OPEN_MAX_AGE_SECS)
        .await;
}

/// 「检查更新」（「应用」页头，ADR-170 ③，PRD-18-011）：用户点了才联网。刷新仍是**同一个动作**——
/// `Catalogs::refresh` 把这台服务端的几份目录一起拉一遍（TDD-14 §3.5「刷新入口只有一个」，与「刷新目录」同一条路）；
/// 这里只按调用方问的那几份目录各自的状态判这一次查没查成：组刷新只回模型目录的状态，别的那份被拒时它照样是 `Ok`。
/// 拉到新的一份时目录观察者广播 `themes://changed` / `templates://changed`，各页签据此重读。
///
/// # Errors
/// `CONTENT_MARKET_UNAVAILABLE`（未登录 / 严格本地 / 没有云插件）、`STRICT_LOCAL_BLOCKED`（刷新前被严格本地拦下）、
/// `CONTENT_TRUST_CHANGED`（验签失败而服务器换了身份，去账户页核对，ADR-060）、
/// `CONTENT_CHECK_FAILED: <目录>:<原因码>`（连不上、验签不过等；缓存照旧可用）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn check_content_updates(
    app: tauri::AppHandle,
    catalogs: Vec<ContentCatalogDto>,
) -> Result<(), String> {
    if !crate::cloud::evaluate_for(&app).allowed() {
        return Err("CONTENT_MARKET_UNAVAILABLE".to_owned());
    }
    let clients = app
        .try_state::<luna_cloud::commands::Catalogs>()
        .map(|catalogs| catalogs.inner().clone())
        .ok_or_else(|| "CONTENT_MARKET_UNAVAILABLE".to_owned())?;
    let cloud = app
        .try_state::<luna_cloud::commands::CloudState>()
        .map(|cloud| Arc::clone(&cloud.0))
        .ok_or_else(|| "CONTENT_MARKET_UNAVAILABLE".to_owned())?;
    match clients.refresh_checking_trust(&cloud).await {
        Ok(true) => Err("CONTENT_TRUST_CHANGED".to_owned()),
        Ok(false) => catalogs.iter().try_for_each(|catalog| {
            let client = match catalog {
                ContentCatalogDto::Themes => &clients.themes,
                ContentCatalogDto::Templates => &clients.templates,
            };
            check_outcome(*catalog, client.status().last_error.as_deref())
        }),
        Err(luna_cloud::CloudError::StrictLocalBlocked) => Err("STRICT_LOCAL_BLOCKED".to_owned()),
        Err(error) => Err(format!("CONTENT_CHECK_FAILED: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 刷新失败在目录状态机里不是 Err：「检查更新」按原因码判成败——连不上、验签不过都要说没查成（2026-10-22
    /// 断网真机：此前一律当成功，页上什么也不说），服务端没发布那份目录只是没有更新。说明里带是哪一份目录。
    #[test]
    fn a_check_fails_on_any_rejection_except_an_unpublished_catalog() {
        for catalog in [ContentCatalogDto::Themes, ContentCatalogDto::Templates] {
            assert!(check_outcome(catalog, None).is_ok());
            assert!(check_outcome(catalog, Some("no_active_catalog")).is_ok());
        }
        for reason in [
            "network",
            "bad_signature",
            "expired",
            "rollback",
            "server_error",
        ] {
            assert_eq!(
                check_outcome(ContentCatalogDto::Templates, Some(reason)),
                Err(format!("CONTENT_CHECK_FAILED: templates:{reason}"))
            );
        }
        assert_eq!(
            check_outcome(ContentCatalogDto::Themes, Some("network")),
            Err("CONTENT_CHECK_FAILED: themes:network".to_owned())
        );
    }
}
