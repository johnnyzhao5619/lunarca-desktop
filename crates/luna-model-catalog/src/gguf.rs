//! GGUF 文件头（ADR-144 ③）：只读键值元数据，不读张量。
//!
//! 导入与安装后都靠它回答「这是什么模型、能不能跑」：架构（随包引擎认不认）、训练上下文、
//! 嵌入模型的池化方式、有没有对话模板。服务端发布链冻结前也用它核对条目声明
//! （[`check_for_release`]，ADR-145 ⑤ A2）——所以它住在这个两端共用的叶子 crate 里：
//! 只读调用方给的 `Read`，自己不开文件。
//!
//! 文件头里最大的是词表数组（十几万个字符串）：一律按长度跳过，不分配；所有长度与计数都有
//! 上限，畸形或恶意文件读到上限即报错，不会把内存吃满。

use std::collections::HashMap;
use std::io::{self, Read};

/// 读出来的、我们用得上的元数据。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GgufHeader {
    /// 格式版本（2 或 3）。
    pub version: u32,
    /// 张量数。
    pub tensor_count: u64,
    /// `general.architecture`（如 `qwen35`、`hunyuan-dense`、`bert`）。
    pub architecture: Option<String>,
    /// `general.name`。
    pub name: Option<String>,
    /// `<arch>.context_length`：训练上下文。
    pub context_length: Option<u64>,
    /// `<arch>.block_count`：层数。
    pub block_count: Option<u64>,
    /// `<arch>.pooling_type`：嵌入模型的池化（见 [`Pooling`]）。
    pub pooling: Option<Pooling>,
    /// `general.file_type`：量化类型编号（llama.cpp `LLAMA_FTYPE_*`）。
    pub file_type: Option<u64>,
    /// 是否自带对话模板（`tokenizer.chat_template`）。
    pub has_chat_template: bool,
}

/// GGUF 的池化编号（llama.cpp `LLAMA_POOLING_TYPE_*`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pooling {
    /// 不池化。
    None,
    /// 均值。
    Mean,
    /// `[CLS]`。
    Cls,
    /// 最后一个 token。
    Last,
    /// 重排序。
    Rank,
}

/// 解析失败。
#[derive(Debug)]
pub enum GgufError {
    /// 不是 GGUF（魔数不对）。
    NotGguf,
    /// 不支持的版本。
    UnsupportedVersion(u32),
    /// 结构不合法（未知类型、长度或计数越界）。
    Malformed(&'static str),
    /// 读不下去（含文件被截断）。
    Io(io::Error),
}

impl std::fmt::Display for GgufError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotGguf => f.write_str("not a GGUF file"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported GGUF version {v}"),
            Self::Malformed(why) => write!(f, "malformed GGUF header: {why}"),
            Self::Io(e) => write!(f, "GGUF header unreadable: {e}"),
        }
    }
}

impl std::error::Error for GgufError {}

impl From<io::Error> for GgufError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

const MAX_KV: u64 = 1 << 16;
const MAX_KEY_LEN: u64 = 1 << 16;
/// 单个字符串值的上限（对话模板几十 KB，留足余量）。
const MAX_STRING_LEN: u64 = 16 << 20;
const MAX_ARRAY_LEN: u64 = 1 << 26;
/// 只把这么短的字符串留下来（架构名、模型名）；更长的只跳过。
const KEEP_STRING_LEN: u64 = 1024;

enum Value {
    Uint(u64),
    Str(String),
    Other,
}

/// 读文件头。读到键值元数据结束为止，不读张量信息。
///
/// # Errors
/// 见 [`GgufError`]。
pub fn read_header(reader: &mut impl Read) -> Result<GgufHeader, GgufError> {
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic)?;
    if &magic != b"GGUF" {
        return Err(GgufError::NotGguf);
    }
    let version = read_u32(reader)?;
    if !(2..=3).contains(&version) {
        return Err(GgufError::UnsupportedVersion(version));
    }
    let tensor_count = read_u64(reader)?;
    let kv_count = read_u64(reader)?;
    if kv_count > MAX_KV {
        return Err(GgufError::Malformed("too many metadata entries"));
    }

    let mut values: HashMap<String, Value> = HashMap::new();
    let mut has_chat_template = false;
    for _ in 0..kv_count {
        let key_len = read_u64(reader)?;
        if key_len > MAX_KEY_LEN {
            return Err(GgufError::Malformed("key too long"));
        }
        let key = read_string_bytes(reader, key_len)?;
        let value_type = read_u32(reader)?;
        if key == "tokenizer.chat_template" {
            has_chat_template = true;
        }
        let value = read_value(reader, value_type)?;
        values.insert(key, value);
    }

    let text = |key: &str| match values.get(key) {
        Some(Value::Str(s)) => Some(s.clone()),
        _ => None,
    };
    let uint = |key: &str| match values.get(key) {
        Some(Value::Uint(v)) => Some(*v),
        _ => None,
    };
    let architecture = text("general.architecture");
    let arch_uint = |suffix: &str| {
        architecture
            .as_deref()
            .and_then(|a| uint(&format!("{a}.{suffix}")))
    };
    Ok(GgufHeader {
        version,
        tensor_count,
        name: text("general.name"),
        context_length: arch_uint("context_length"),
        block_count: arch_uint("block_count"),
        pooling: arch_uint("pooling_type").and_then(|p| match p {
            0 => Some(Pooling::None),
            1 => Some(Pooling::Mean),
            2 => Some(Pooling::Cls),
            3 => Some(Pooling::Last),
            4 => Some(Pooling::Rank),
            _ => None,
        }),
        file_type: uint("general.file_type"),
        has_chat_template,
        architecture,
    })
}

fn read_value(reader: &mut impl Read, value_type: u32) -> Result<Value, GgufError> {
    Ok(match value_type {
        0 | 1 | 7 => Value::Uint(u64::from(read_n::<1>(reader)?[0])),
        2 | 3 => Value::Uint(u64::from(u16::from_le_bytes(read_n(reader)?))),
        4 | 5 => Value::Uint(u64::from(read_u32(reader)?)),
        6 => {
            skip(reader, 4)?;
            Value::Other
        }
        10 | 11 => Value::Uint(read_u64(reader)?),
        12 => {
            skip(reader, 8)?;
            Value::Other
        }
        8 => {
            let len = read_u64(reader)?;
            if len > MAX_STRING_LEN {
                return Err(GgufError::Malformed("string value too long"));
            }
            if len <= KEEP_STRING_LEN {
                Value::Str(read_string_bytes(reader, len)?)
            } else {
                skip(reader, len)?;
                Value::Other
            }
        }
        9 => {
            let elem_type = read_u32(reader)?;
            let count = read_u64(reader)?;
            if count > MAX_ARRAY_LEN {
                return Err(GgufError::Malformed("array too long"));
            }
            match fixed_size(elem_type) {
                Some(size) => skip(reader, count * size)?,
                None if elem_type == 8 => {
                    for _ in 0..count {
                        let len = read_u64(reader)?;
                        if len > MAX_STRING_LEN {
                            return Err(GgufError::Malformed("array string too long"));
                        }
                        skip(reader, len)?;
                    }
                }
                None => return Err(GgufError::Malformed("unsupported array element type")),
            }
            Value::Other
        }
        _ => return Err(GgufError::Malformed("unknown value type")),
    })
}

fn fixed_size(value_type: u32) -> Option<u64> {
    match value_type {
        0 | 1 | 7 => Some(1),
        2 | 3 => Some(2),
        4..=6 => Some(4),
        10..=12 => Some(8),
        _ => None,
    }
}

fn read_n<const N: usize>(reader: &mut impl Read) -> io::Result<[u8; N]> {
    let mut buf = [0u8; N];
    reader.read_exact(&mut buf)?;
    Ok(buf)
}

fn read_u32(reader: &mut impl Read) -> io::Result<u32> {
    Ok(u32::from_le_bytes(read_n(reader)?))
}

fn read_u64(reader: &mut impl Read) -> io::Result<u64> {
    Ok(u64::from_le_bytes(read_n(reader)?))
}

fn read_string_bytes(reader: &mut impl Read, len: u64) -> Result<String, GgufError> {
    let mut bytes = Vec::new();
    reader.take(len).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != len {
        return Err(GgufError::Io(io::ErrorKind::UnexpectedEof.into()));
    }
    String::from_utf8(bytes).map_err(|_| GgufError::Malformed("string is not UTF-8"))
}

fn skip(reader: &mut impl Read, len: u64) -> Result<(), GgufError> {
    let copied = io::copy(&mut reader.take(len), &mut io::sink())?;
    if copied != len {
        return Err(GgufError::Io(io::ErrorKind::UnexpectedEof.into()));
    }
    Ok(())
}

/// `general.file_type`（llama.cpp `LLAMA_FTYPE_*`）→ 目录 `quant` 的写法。只列目录会发的档；
/// 不在表里的编号核对时按「认不出」处理（冻结前拒，见 [`check_for_release`]）。
#[must_use]
pub fn quant_name(file_type: u64) -> Option<&'static str> {
    Some(match file_type {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        7 => "Q8_0",
        8 => "Q5_0",
        12 => "Q3_K_M",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        32 => "BF16",
        _ => return None,
    })
}

/// 随包引擎**实测跑通过**的架构，及跑通时的引擎构建号（ADR-145 ⑤ A2）。
///
/// 这是发布纪律，不是 llama.cpp 的支持表：一条目录条目要发出去，它的架构必须有人在不高于
/// 其 `engine_min_build` 的构建上真跑过（`luna model-eval`）。新架构 = 先实测、再在这里加一行、
/// 服务端升 pin 后才能冻结。构建号与 `luna-engine` 的 `runtime.rs` 钉住的引擎同源，升引擎时
/// 不必改这里——表里记的是「最早在哪个构建上验过」。
pub const VERIFIED_ARCHITECTURES: &[(&str, u32)] = &[
    ("bert", 11_429),
    ("hunyuan-dense", 11_429),
    ("qwen3", 11_429),
    ("qwen35", 11_429),
];

/// 文件头与条目声明是否一致：安装落位前（客户端）与冻结前（服务端，经 [`check_for_release`]）
/// 共用。sha256 只证明「下到的是目录说的那个文件」，证明不了目录本身写对了——角色写反
/// （把嵌入模型登记成对话）会让引擎以错误参数加载。
///
/// # Errors
/// 不一致时返回英文原因（客户端拼进稳定错误码的明细，服务端原样回给运营者）。
pub fn check_entry(header: &GgufHeader, gguf: &crate::Gguf) -> Result<(), String> {
    use crate::{GgufRole, Pooling as Declared};
    let (Some(_), Some(trained)) = (&header.architecture, header.context_length) else {
        return Err("missing architecture or context length".to_owned());
    };
    if u64::from(gguf.max_context) > trained {
        return Err(format!(
            "max_context {} exceeds the trained context {trained}",
            gguf.max_context
        ));
    }
    match gguf.role {
        GgufRole::Chat { .. } if !header.has_chat_template => {
            Err("chat model has no chat template".to_owned())
        }
        GgufRole::Chat { .. } if header.pooling.is_some_and(|p| p != Pooling::None) => {
            Err("listed as chat but the file is an embedding model".to_owned())
        }
        GgufRole::Chat { .. } => Ok(()),
        GgufRole::Embed { pooling, .. } => {
            let expected = match pooling {
                Declared::Cls => Pooling::Cls,
                Declared::Last => Pooling::Last,
                Declared::Mean => Pooling::Mean,
            };
            if header.pooling == Some(expected) {
                Ok(())
            } else {
                Err("embedding pooling differs from the catalog".to_owned())
            }
        }
    }
}

/// 冻结前的完整核对（ADR-145 ⑤ A2）：[`check_entry`] 之外，再要求量化档与文件一致、
/// 架构在 [`VERIFIED_ARCHITECTURES`] 里且条目的 `engine_min_build` 不低于实测构建。
/// 客户端安装不跑这两条：旧客户端的表必然比新目录旧，拿它拒新架构只会误伤。
///
/// # Errors
/// 同 [`check_entry`]。
pub fn check_for_release(header: &GgufHeader, gguf: &crate::Gguf) -> Result<(), String> {
    check_entry(header, gguf)?;
    let actual = header.file_type.and_then(quant_name);
    if actual != Some(gguf.quant.as_str()) {
        return Err(format!(
            "quant {} differs from the file ({})",
            gguf.quant,
            actual.map_or_else(
                || format!("file_type {:?}", header.file_type),
                str::to_owned
            )
        ));
    }
    let arch = header.architecture.as_deref().unwrap_or_default();
    match VERIFIED_ARCHITECTURES
        .iter()
        .find(|(name, _)| *name == arch)
    {
        None => Err(format!(
            "architecture {arch} has not been verified on the bundled engine"
        )),
        Some((_, verified)) if gguf.engine_min_build < *verified => Err(format!(
            "engine_min_build {} is below build {verified}, the first one {arch} was verified on",
            gguf.engine_min_build
        )),
        Some(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 拼一个最小 GGUF 头。
    struct Builder(Vec<u8>, u64);

    impl Builder {
        fn new() -> Self {
            Self(Vec::new(), 0)
        }
        fn key(&mut self, key: &str, ty: u32) -> &mut Self {
            self.0.extend_from_slice(&(key.len() as u64).to_le_bytes());
            self.0.extend_from_slice(key.as_bytes());
            self.0.extend_from_slice(&ty.to_le_bytes());
            self.1 += 1;
            self
        }
        fn string(&mut self, key: &str, value: &str) -> &mut Self {
            self.key(key, 8);
            self.0
                .extend_from_slice(&(value.len() as u64).to_le_bytes());
            self.0.extend_from_slice(value.as_bytes());
            self
        }
        fn u32(&mut self, key: &str, value: u32) -> &mut Self {
            self.key(key, 4);
            self.0.extend_from_slice(&value.to_le_bytes());
            self
        }
        fn string_array(&mut self, key: &str, items: &[&str]) -> &mut Self {
            self.key(key, 9);
            self.0.extend_from_slice(&8u32.to_le_bytes());
            self.0
                .extend_from_slice(&(items.len() as u64).to_le_bytes());
            for item in items {
                self.0.extend_from_slice(&(item.len() as u64).to_le_bytes());
                self.0.extend_from_slice(item.as_bytes());
            }
            self
        }
        fn f32_array(&mut self, key: &str, n: u64) -> &mut Self {
            self.key(key, 9);
            self.0.extend_from_slice(&6u32.to_le_bytes());
            self.0.extend_from_slice(&n.to_le_bytes());
            self.0.extend(std::iter::repeat_n(0u8, (n * 4) as usize));
            self
        }
        fn bytes(&self) -> Vec<u8> {
            let mut out = b"GGUF".to_vec();
            out.extend_from_slice(&3u32.to_le_bytes());
            out.extend_from_slice(&7u64.to_le_bytes());
            out.extend_from_slice(&self.1.to_le_bytes());
            out.extend_from_slice(&self.0);
            out.extend_from_slice(b"TENSOR-DATA-NOT-READ");
            out
        }
    }

    #[test]
    fn reads_the_fields_we_use_and_skips_the_vocabulary() {
        let bytes = Builder::new()
            .string("general.architecture", "bert")
            .string("general.name", "bge-m3")
            .u32("bert.context_length", 8192)
            .u32("bert.block_count", 24)
            .u32("bert.pooling_type", 2)
            .u32("general.file_type", 7)
            .string_array("tokenizer.ggml.tokens", &["<s>", "</s>", "hello"])
            .f32_array("tokenizer.ggml.scores", 3)
            .string("tokenizer.chat_template", "{{ messages }}")
            .bytes();
        let header = read_header(&mut bytes.as_slice()).unwrap();
        assert_eq!(header.version, 3);
        assert_eq!(header.tensor_count, 7);
        assert_eq!(header.architecture.as_deref(), Some("bert"));
        assert_eq!(header.name.as_deref(), Some("bge-m3"));
        assert_eq!(header.context_length, Some(8192));
        assert_eq!(header.block_count, Some(24));
        assert_eq!(header.pooling, Some(Pooling::Cls));
        assert_eq!(header.file_type, Some(7));
        assert!(header.has_chat_template);
    }

    #[test]
    fn arch_scoped_keys_resolve_even_when_architecture_comes_last() {
        let bytes = Builder::new()
            .u32("qwen35.context_length", 262_144)
            .string("general.architecture", "qwen35")
            .bytes();
        let header = read_header(&mut bytes.as_slice()).unwrap();
        assert_eq!(header.context_length, Some(262_144));
        assert!(!header.has_chat_template);
    }

    #[test]
    fn rejects_non_gguf_truncated_and_hostile_inputs() {
        assert!(matches!(
            read_header(&mut &b"GGML\x03\0\0\0"[..]),
            Err(GgufError::NotGguf)
        ));
        let mut v1 = b"GGUF".to_vec();
        v1.extend_from_slice(&1u32.to_le_bytes());
        assert!(matches!(
            read_header(&mut v1.as_slice()),
            Err(GgufError::UnsupportedVersion(1))
        ));

        let full = Builder::new()
            .string("general.architecture", "bert")
            .bytes();
        let truncated = &full[..full.len() - 30];
        assert!(matches!(
            read_header(&mut &truncated[..]),
            Err(GgufError::Io(_))
        ));

        // 声称有 2^40 个键：读到计数就拒绝，不去循环
        let mut hostile = b"GGUF".to_vec();
        hostile.extend_from_slice(&3u32.to_le_bytes());
        hostile.extend_from_slice(&0u64.to_le_bytes());
        hostile.extend_from_slice(&(1u64 << 40).to_le_bytes());
        assert!(matches!(
            read_header(&mut hostile.as_slice()),
            Err(GgufError::Malformed(_))
        ));

        // 一个声称 1 TB 长的字符串值
        let mut huge = Builder::new();
        huge.key("general.name", 8);
        huge.0.extend_from_slice(&(1u64 << 40).to_le_bytes());
        assert!(matches!(
            read_header(&mut huge.bytes().as_slice()),
            Err(GgufError::Malformed(_))
        ));
    }

    /// 真文件（P0 下载的那批）：`LUNA_GGUF_DIR=~/Dev/lunarca-p0/models cargo test -p luna-model-fetch -- --ignored`
    #[test]
    #[ignore = "需要本机 GGUF 文件（见注释）"]
    fn real_files_from_the_p0_set() {
        let dir = std::path::PathBuf::from(std::env::var("LUNA_GGUF_DIR").expect("LUNA_GGUF_DIR"));
        let read = |name: &str| {
            let mut file = std::io::BufReader::new(std::fs::File::open(dir.join(name)).unwrap());
            read_header(&mut file).unwrap()
        };
        for (name, arch, template, pooling) in [
            ("qwen3.5-4b-q4_k_m.gguf", None, true, None),
            ("hy-mt2-1.8b-q8_0.gguf", None, true, None),
            ("bge-m3-q8_0.gguf", Some("bert"), false, Some(Pooling::Cls)),
            (
                "qwen3-embedding-0.6b-q8_0.gguf",
                None,
                true,
                Some(Pooling::Last),
            ),
        ] {
            let h = read(name);
            eprintln!("{name}: {h:?}");
            if let Some(arch) = arch {
                assert_eq!(h.architecture.as_deref(), Some(arch), "{name}");
            }
            assert!(
                h.architecture.is_some() && h.context_length.is_some(),
                "{name}"
            );
            assert_eq!(h.has_chat_template, template, "{name}");
            if pooling.is_some() {
                assert_eq!(h.pooling, pooling, "{name}");
            }
        }
    }

    /// 随包目录每条 GGUF 条目都要过冻结前核对（文件名 = 条目 id 去掉 `-gguf`）。
    #[test]
    #[ignore = "需要本机 GGUF 文件（见 real_files_from_the_p0_set）"]
    fn bundled_entries_pass_the_release_check_against_real_files() {
        let dir = std::path::PathBuf::from(std::env::var("LUNA_GGUF_DIR").expect("LUNA_GGUF_DIR"));
        let mut checked = 0;
        for entry in crate::bundled().iter().filter(|e| e.gguf.is_some()) {
            let path = dir.join(format!("{}.gguf", entry.id.trim_end_matches("-gguf")));
            if !path.is_file() {
                continue;
            }
            let header = read_header(&mut std::io::BufReader::new(
                std::fs::File::open(&path).unwrap(),
            ))
            .unwrap();
            check_for_release(&header, entry.gguf.as_ref().unwrap())
                .unwrap_or_else(|why| panic!("{}: {why}", entry.id));
            checked += 1;
        }
        assert!(checked > 0, "no bundled GGUF found under {}", dir.display());
    }

    fn declared(role: crate::GgufRole, quant: &str, max_context: u32, build: u32) -> crate::Gguf {
        crate::Gguf {
            engine_min_build: build,
            quant: quant.to_owned(),
            max_context,
            role,
            translation_only: false,
        }
    }

    fn parsed(arch: &str, file_type: u64, pooling: Option<Pooling>, template: bool) -> GgufHeader {
        GgufHeader {
            version: 3,
            architecture: Some(arch.to_owned()),
            context_length: Some(32_768),
            pooling,
            file_type: Some(file_type),
            has_chat_template: template,
            ..GgufHeader::default()
        }
    }

    const CHAT: crate::GgufRole = crate::GgufRole::Chat { thinking: true };

    #[test]
    fn release_check_accepts_a_matching_verified_entry() {
        let header = parsed("qwen35", 15, Some(Pooling::None), true);
        assert_eq!(
            check_for_release(&header, &declared(CHAT, "Q4_K_M", 16_384, 11_429)),
            Ok(())
        );
        let embed = crate::GgufRole::Embed {
            pooling: crate::Pooling::Last,
            encoder: false,
        };
        let header = parsed("qwen3", 7, Some(Pooling::Last), true);
        assert_eq!(
            check_for_release(&header, &declared(embed, "Q8_0", 8192, 11_429)),
            Ok(())
        );
    }

    #[test]
    fn release_check_rejects_what_the_install_check_lets_through() {
        let header = parsed("qwen35", 15, None, true);
        // 量化档写错：安装时放行（客户端不判），冻结前拒。
        let wrong_quant = declared(CHAT, "Q8_0", 16_384, 11_429);
        assert_eq!(check_entry(&header, &wrong_quant), Ok(()));
        assert!(check_for_release(&header, &wrong_quant)
            .unwrap_err()
            .contains("quant Q8_0"));
        // 认不出的量化编号也拒。
        assert!(
            check_for_release(&parsed("qwen35", 99, None, true), &wrong_quant)
                .unwrap_err()
                .contains("file_type Some(99)")
        );
        // 没实测过的架构。
        assert!(check_for_release(
            &parsed("llama4", 15, None, true),
            &declared(CHAT, "Q4_K_M", 8192, 99_999)
        )
        .unwrap_err()
        .contains("not been verified"));
        // 引擎下限低于实测构建。
        assert!(
            check_for_release(&header, &declared(CHAT, "Q4_K_M", 8192, 11_000))
                .unwrap_err()
                .contains("below build 11429")
        );
    }

    #[test]
    fn entry_check_rejects_a_context_beyond_training() {
        let header = parsed("qwen35", 15, None, true);
        assert!(
            check_entry(&header, &declared(CHAT, "Q4_K_M", 65_536, 11_429))
                .unwrap_err()
                .contains("exceeds the trained context 32768")
        );
        let mut no_ctx = header;
        no_ctx.context_length = None;
        assert!(check_entry(&no_ctx, &declared(CHAT, "Q4_K_M", 8192, 11_429)).is_err());
    }
}
