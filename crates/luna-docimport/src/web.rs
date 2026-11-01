//! 网页剪藏的正文抽取（ADR-164 ⑦）：一页 HTML → 文章正文 HTML + 元数据。
//!
//! 纯函数：不联网。抓取、图片下载与落盘都在调用方；这里只认出「哪段是正文、标题作者站点是什么、
//! 规范链接是哪个、正文里有哪些图片」。正文 HTML 由调用方改写好图片地址后再经
//! [`crate::convert_html_document`] 转成 Markdown。

use dom_query::Document;

use crate::DocImportError;

/// 正文少于这么多个可见字符就当作没有文章（只剩导航、登录墙、空壳页）。
const MIN_ARTICLE_CHARS: usize = 50;

/// 抽出来的一篇文章。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebArticle {
    pub title: String,
    pub author: Option<String>,
    /// 发布时间（原样，网页给什么就是什么，通常是 ISO 8601）。
    pub published: Option<String>,
    pub site: Option<String>,
    /// 规范链接：`<link rel=canonical>` → `og:url` → 抓取到的最终地址，去掉追踪参数。
    pub canonical_url: String,
    /// 正文 HTML（相对链接与图片地址已按页面地址补成绝对地址）。
    pub content_html: String,
    /// 正文里的图片（绝对地址，按出现顺序、去重）。
    pub images: Vec<String>,
}

/// 从一页 HTML 里抽出文章。`final_url` 是抓取跟完重定向后的地址。
///
/// # Errors
/// 认不出正文时返回 [`DocImportError::Empty`]。
pub fn extract_article(html: &str, final_url: &str) -> Result<WebArticle, DocImportError> {
    let page = Document::from(html);
    let meta = |selector: &str| {
        page.select(selector)
            .attr("content")
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let canonical = page
        .select("link[rel=canonical]")
        .attr("href")
        .map(|v| v.trim().to_owned())
        .filter(|v| v.starts_with("http://") || v.starts_with("https://"))
        .or_else(|| meta("meta[property='og:url']"))
        .filter(|v| v.starts_with("http://") || v.starts_with("https://"))
        .unwrap_or_else(|| final_url.to_owned());

    // 懒加载图片：`src` 只是一张占位图，真图在 `data-src` 之类的属性里（真机 2026-10-19：新浪博客
    // 用 `real_src`，下回来的是一张 1×1 透明 gif）。先换回真图地址再交给 Readability。
    let html = with_lazy_images_resolved(html);
    let mut readability = dom_smoothie::Readability::new(html.as_str(), Some(final_url), None)
        .map_err(|_| DocImportError::Empty)?;
    let article = readability.parse().map_err(|_| DocImportError::Empty)?;
    // Readability 认不出正文时会把导航之类的残片当成文章交回来：正文少于这么多字就当没有。
    if article
        .text_content
        .chars()
        .filter(|c| !c.is_whitespace())
        .count()
        < MIN_ARTICLE_CHARS
    {
        return Err(DocImportError::Empty);
    }

    let site_hint = meta("meta[property='og:site_name']").or_else(|| host_of(&canonical));
    let title = meta("meta[property='og:title']")
        .or_else(|| Some(article.title.trim().to_owned()).filter(|t| !t.is_empty()))
        .or_else(|| {
            Some(page.select("head > title").text().trim().to_owned()).filter(|t| !t.is_empty())
        })
        .map(|title| without_site_suffix(&title, site_hint.as_deref()))
        .unwrap_or_default();
    let author = article
        .byline
        .clone()
        .map(|v| strip_byline_label(&v))
        .filter(|v| !v.is_empty())
        .or_else(|| meta("meta[name=author]"))
        .or_else(|| meta("meta[property='article:author']"));
    let published = meta("meta[property='article:published_time']").or_else(|| {
        article
            .published_time
            .clone()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    });
    let site = meta("meta[property='og:site_name']")
        .or_else(|| article.site_name.clone().filter(|v| !v.trim().is_empty()))
        .or_else(|| host_of(&canonical));

    let content_html = article.content.to_string();
    let content = Document::from(content_html.as_str());
    let mut images: Vec<String> = Vec::new();
    for image in content.select("img[src]").nodes() {
        if let Some(src) = image.attr("src") {
            let src = src.trim().to_owned();
            if (src.starts_with("http://") || src.starts_with("https://")) && !images.contains(&src)
            {
                images.push(src);
            }
        }
    }
    Ok(WebArticle {
        title,
        author,
        published,
        site,
        canonical_url: strip_tracking(&canonical),
        content_html,
        images,
    })
}

/// 把正文里已下载的图片地址换成附件相对路径（`map`：原地址 → 链接目标）；没下载的原样留着，
/// 之后转换时只剩替代文字。
#[must_use]
pub fn rewrite_images(content_html: &str, map: &[(String, String)]) -> String {
    if map.is_empty() {
        return content_html.to_owned();
    }
    let doc = Document::from(content_html);
    for image in doc.select("img[src]").nodes() {
        let Some(src) = image.attr("src") else {
            continue;
        };
        if let Some((_, target)) = map.iter().find(|(from, _)| from.as_str() == src.trim()) {
            image.set_attr("src", target);
        }
    }
    doc.select("body").inner_html().to_string()
}

/// 去掉常见的追踪参数（`utm_*`、`fbclid`、`gclid`、`spm` 等）与片段；剩下的参数原样保留顺序。
#[must_use]
pub fn strip_tracking(url: &str) -> String {
    let url = url.split('#').next().unwrap_or(url);
    let Some((base, query)) = url.split_once('?') else {
        return url.to_owned();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter(|pair| {
            let key = pair.split('=').next().unwrap_or("").to_ascii_lowercase();
            !(key.starts_with("utm_")
                || matches!(
                    key.as_str(),
                    "fbclid"
                        | "gclid"
                        | "dclid"
                        | "msclkid"
                        | "mc_cid"
                        | "mc_eid"
                        | "spm"
                        | "share_source"
                        | "share_medium"
                        | "share_from"
                        | "vd_source"
                        | "igshid"
                        | "ref_src"
                ))
        })
        .collect();
    if kept.is_empty() {
        base.to_owned()
    } else {
        format!("{base}?{}", kept.join("&"))
    }
}

/// 懒加载图片的真图属性（按常见程度排）。
const LAZY_SRC_ATTRS: [&str; 6] = [
    "data-src",
    "data-original",
    "data-lazy-src",
    "data-actualsrc",
    "real_src",
    "data-url",
];

/// 把 `<img>` 的懒加载真图地址换进 `src`（没有就原样）。
fn with_lazy_images_resolved(html: &str) -> String {
    let doc = Document::from(html);
    let mut changed = false;
    for image in doc.select("img").nodes() {
        let lazy = LAZY_SRC_ATTRS.iter().find_map(|attr| {
            image
                .attr(attr)
                .map(|v| v.trim().to_owned())
                .filter(|v| v.starts_with("http") || v.starts_with("//") || v.starts_with('/'))
        });
        if let Some(src) = lazy {
            image.set_attr("src", &src);
            changed = true;
        }
    }
    if changed {
        doc.html().to_string()
    } else {
        html.to_owned()
    }
}

/// 标题末尾常挂着站点名（「文章 | 站点」「文章 - 站点」）：最后一段就是站点名时去掉。
fn without_site_suffix(title: &str, site: Option<&str>) -> String {
    let title = title.trim();
    let Some(site) = site else {
        return title.to_owned();
    };
    let norm = |s: &str| {
        s.trim_start_matches("www.")
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    let site = norm(site);
    for sep in [" | ", " - ", " — ", " – ", " · ", "_"] {
        if let Some((head, tail)) = title.rsplit_once(sep) {
            let tail = norm(tail);
            if !head.trim().is_empty()
                && !tail.is_empty()
                && (site.contains(&tail) || tail.contains(&site))
            {
                return head.trim().to_owned();
            }
        }
    }
    title.to_owned()
}

/// 署名常带一个「作者：」「By」之类的前缀：只留名字。
fn strip_byline_label(byline: &str) -> String {
    let byline = byline.trim();
    for label in [
        "作者：",
        "作者:",
        "作者",
        "文/",
        "文 /",
        "By ",
        "by ",
        "BY ",
    ] {
        if let Some(rest) = byline.strip_prefix(label) {
            return rest.trim().to_owned();
        }
    }
    byline.to_owned()
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = host.split(':').next()?.trim_start_matches("www.");
    (!host.is_empty()).then(|| host.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<!doctype html><html><head>
        <title>页面标题 - 某站</title>
        <link rel="canonical" href="https://example.com/post/1?utm_source=x&id=7">
        <meta property="og:title" content="把脚本写顺口的三个办法">
        <meta property="og:site_name" content="某站">
        <meta name="author" content="张三">
        <meta property="article:published_time" content="2026-10-01T08:00:00+08:00">
        </head><body>
        <nav><a href="/">首页</a> <a href="/about">关于</a></nav>
        <article>
          <h1>把脚本写顺口的三个办法</h1>
          <p>写视频脚本最常见的问题，是把书面语直接念出来。第一，句子要短；第二，少用倒装；第三，念一遍再改。</p>
          <p>很多人写完稿子一念，才发现句子太长、嘴跟不上。解决办法是每句话控制在一口气能说完的长度里。</p>
          <p><img src="/img/a.png" alt="示意图"> <a href="/related">相关文章</a></p>
          <p>最后，记得把口头禅删掉，但保留口语里的停顿，这样观众听起来更自然，也更像在跟人聊天。</p>
        </article>
        <footer>版权所有</footer>
        </body></html>"#;

    #[test]
    fn article_metadata_and_images() {
        let article =
            extract_article(PAGE, "https://example.com/post/1?utm_source=x&id=7").unwrap();
        assert_eq!(article.title, "把脚本写顺口的三个办法");
        assert_eq!(article.author.as_deref(), Some("张三"));
        assert_eq!(
            article.published.as_deref(),
            Some("2026-10-01T08:00:00+08:00")
        );
        assert_eq!(article.site.as_deref(), Some("某站"));
        assert_eq!(article.canonical_url, "https://example.com/post/1?id=7");
        assert!(
            article.content_html.contains("一口气能说完"),
            "{}",
            article.content_html
        );
        assert!(!article.content_html.contains("版权所有"));
        assert_eq!(
            article.images,
            vec!["https://example.com/img/a.png".to_owned()]
        );
    }

    #[test]
    fn downloaded_images_point_at_attachments() {
        let html = r#"<p>前<img src="https://e.com/a.png" alt="甲"><img src="https://e.com/b.png" alt="乙">后</p>"#;
        let out = rewrite_images(
            html,
            &[("https://e.com/a.png".into(), "assets/a-1.png".into())],
        );
        assert!(out.contains(r#"src="assets/a-1.png""#), "{out}");
        assert!(out.contains(r#"src="https://e.com/b.png""#), "{out}");
        let md = crate::convert_html_document(&out, true);
        assert!(
            md.markdown.contains("![甲](assets/a-1.png)"),
            "{}",
            md.markdown
        );
        assert!(!md.markdown.contains("b.png"), "{}", md.markdown);
    }

    #[test]
    fn tracking_parameters_and_fragments_are_dropped() {
        assert_eq!(
            strip_tracking("https://a.com/x?utm_medium=y&fbclid=1&page=2#top"),
            "https://a.com/x?page=2"
        );
        assert_eq!(
            strip_tracking("https://b.com/v?spm=1&vd_source=2"),
            "https://b.com/v"
        );
        assert_eq!(strip_tracking("https://c.com/plain"), "https://c.com/plain");
    }

    #[test]
    fn lazy_images_use_their_real_source() {
        let page = r#"<html><body><article><h1>标题</h1>
            <p>正文第一段，讲怎么在低成交量的时候做短线，注意仓位与节奏，别追高也别恐慌割肉，要有耐心等机会。</p>
            <p><img src="//s.cn/trans.gif" real_src="http://img.cn/real.jpg" alt="真图"></p>
            <p>正文第二段，继续讲操作计划与持仓记录，把每天的思路写下来，方便以后复盘对照，形成自己的交易体系。</p>
            </article></body></html>"#;
        let article = extract_article(page, "https://blog.example.cn/p/1.html").unwrap();
        assert_eq!(
            article.images,
            vec!["http://img.cn/real.jpg".to_owned()],
            "{}",
            article.content_html
        );
    }

    #[test]
    fn site_suffix_is_dropped_from_titles() {
        assert_eq!(
            without_site_suffix("Students Down 26% | HousingAI", Some("HousingAI")),
            "Students Down 26%"
        );
        assert_eq!(
            without_site_suffix("房源详情 | Realmaster.com", Some("www.realmaster.com")),
            "房源详情"
        );
        assert_eq!(without_site_suffix("A | B", Some("example.com")), "A | B");
        assert_eq!(without_site_suffix("只有标题", Some("某站")), "只有标题");
    }

    #[test]
    fn byline_labels_are_stripped() {
        assert_eq!(strip_byline_label("作者： 阮一峰"), "阮一峰");
        assert_eq!(strip_byline_label("By Jane Doe"), "Jane Doe");
        assert_eq!(strip_byline_label("张三"), "张三");
    }

    #[test]
    fn site_falls_back_to_host() {
        assert_eq!(
            host_of("https://www.example.org:8443/a?b"),
            Some("example.org".to_owned())
        );
    }

    #[test]
    fn pages_without_text_are_empty() {
        assert_eq!(
            extract_article(
                "<html><body><nav>菜单</nav></body></html>",
                "https://a.com/"
            ),
            Err(DocImportError::Empty)
        );
    }
}
