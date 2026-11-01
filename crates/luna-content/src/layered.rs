//! 「内置 < 内容层 < 库内」分层查找（ADR-159 ④ / ADR-169 ①）：同名时外层优先。
//!
//! 一个纯函数，三个域共用：调用方给出每一层的条目与「怎么取名字」，拿回一份去重后的清单，
//! 每项标出它来自哪一层、被它盖住的有哪几层（界面据此写「你的版本」「来自 XX 包」）。

/// 来源层，由内到外。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    /// 随 Lunarca 发布。
    Builtin,
    /// 内容层：从文件导入或市场安装的。
    Content,
    /// 库内：用户自己写的（随库同步）。
    Vault,
}

/// 分层后的一项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layered<T> {
    pub item: T,
    pub layer: Layer,
    /// 同名、被这一项盖住的那些层（由内到外）。
    pub shadows: Vec<Layer>,
}

/// 把各层的条目合成一份清单：同名时外层（`Vault` > `Content` > `Builtin`）的那一项留下，
/// 顺序按「第一次出现」——先列内置的，再列只在外层才有的。同一层里重名只认第一个。
pub fn layered<T, K: PartialEq>(
    layers: Vec<(Layer, Vec<T>)>,
    key: impl Fn(&T) -> K,
) -> Vec<Layered<T>> {
    let mut layers = layers;
    layers.sort_by_key(|(layer, _)| *layer);
    let mut out: Vec<Layered<T>> = Vec::new();
    for (layer, items) in layers {
        let mut seen_here: Vec<K> = Vec::new();
        for item in items {
            let name = key(&item);
            if seen_here.contains(&name) {
                continue;
            }
            if let Some(existing) = out.iter_mut().find(|entry| key(&entry.item) == name) {
                existing.shadows.push(existing.layer);
                existing.item = item;
                existing.layer = layer;
            } else {
                out.push(Layered {
                    item,
                    layer,
                    shadows: Vec::new(),
                });
            }
            seen_here.push(name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outer_layers_win_and_remember_what_they_shadow() {
        let merged = layered(
            vec![
                (Layer::Vault, vec![("weekly", "mine")]),
                (
                    Layer::Builtin,
                    vec![("note", "builtin"), ("weekly", "builtin")],
                ),
                (
                    Layer::Content,
                    vec![("weekly", "pack"), ("meeting", "pack")],
                ),
            ],
            |(name, _)| *name,
        );
        let view: Vec<(&str, &str, Layer, Vec<Layer>)> = merged
            .iter()
            .map(|entry| {
                (
                    entry.item.0,
                    entry.item.1,
                    entry.layer,
                    entry.shadows.clone(),
                )
            })
            .collect();
        assert_eq!(
            view,
            vec![
                ("note", "builtin", Layer::Builtin, vec![]),
                (
                    "weekly",
                    "mine",
                    Layer::Vault,
                    vec![Layer::Builtin, Layer::Content]
                ),
                ("meeting", "pack", Layer::Content, vec![]),
            ]
        );
    }

    #[test]
    fn duplicates_inside_one_layer_keep_the_first() {
        let merged = layered(
            vec![(Layer::Content, vec![("a", 1), ("a", 2)])],
            |(name, _)| *name,
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].item.1, 1);
    }
}
