use luna_core::JobId;
use std::collections::HashMap;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Debug)]
pub struct MediaServer {
    base_url: String,
    /// 每会话 token（TDD-12 §4 ③）。**共享可变**——与 `root` 完全同构：切库时轮换，
    /// 连接循环每次读当前值，故旧库遗留在前端的 URL 立即失效。
    token: Arc<RwLock<String>>,
    /// 当前 Vault 根（启动时无 Vault → None；选/建 Vault 后由 `set_root` 更新）。
    root: Arc<RwLock<Option<PathBuf>>>,
    /// 多分片 WAV 虚拟拼接注册表（composite_id → 布局；TDD-12 §4）。
    /// 换 Vault 时清空，防止跨 Vault 残留可访问句柄。
    composites: Arc<RwLock<HashMap<String, CompositeLayout>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
    pub status: u16,
}

// ─── 多分片 WAV 虚拟拼接（PRD-04 §5「一条连续录音、时间轴连续」）────────────────
//
// 暂停/继续产生 `-pNN.wav` 多分片；对前端以单一 `?composite=<id>` 资源呈现：
// 合成一个标准 44 字节 WAV 头（data 长度 = 各分片之和），Range 请求经纯函数
// [`map_composite_range`] 映射到「合成头 + 各分片 data 区」的读取切片序列。
// 前提：各分片由自家 `WavEncoder` 写出（PCM、fmt 一致），注册时按实际 RIFF 头校验。
// 已知约束：FLAC 接入后 PCM 直拼前提不再成立，届时需重审（见 TDD-12 §4）。

/// 单个 WAV 分片的解析结果（仅头部元信息，不载样本）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WavInfo {
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    /// data chunk 正文在文件内的字节偏移。
    pub data_offset: u64,
    /// data chunk 正文字节数（来自头部，`finalize` 已回填）。
    pub data_len: u64,
}

/// 虚拟拼接布局：合成头 + 有序分片表。
#[derive(Clone, Debug)]
pub struct CompositeLayout {
    /// 合成的标准 44 字节 WAV 头。
    header: Vec<u8>,
    parts: Vec<CompositePart>,
}

#[derive(Clone, Debug)]
struct CompositePart {
    path: PathBuf,
    data_offset: u64,
    data_len: u64,
}

impl CompositeLayout {
    /// 虚拟资源总长（合成头 + 各分片 data 之和）。
    #[must_use]
    pub fn total_len(&self) -> u64 {
        self.header.len() as u64 + self.parts.iter().map(|p| p.data_len).sum::<u64>()
    }
}

/// Range 映射产出的读取切片（按序写出即为响应体）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompositeSlice {
    /// 从合成头 `[offset, offset+len)` 取字节。
    Header { offset: usize, len: usize },
    /// 从第 `idx` 个分片文件 `file_offset` 起读 `len` 字节。
    Part {
        idx: usize,
        file_offset: u64,
        len: u64,
    },
}

/// 解析 WAV 头（RIFF chunk 遍历，含奇数长度 pad 对齐）。仅接受 PCM。
///
/// # Errors
/// 非 RIFF/WAVE、非 PCM、`fmt `/`data` chunk 缺失或截断时返回错误。
pub fn parse_wav_header(bytes: &[u8]) -> Result<WavInfo, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("非 RIFF/WAVE 文件".into());
    }
    let mut pos = 12usize;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits = 0u16;
    let mut have_fmt = false;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().expect("4 bytes")) as u64;
        let body = pos + 8;
        if id == b"fmt " {
            if bytes.len() < body + 16 {
                return Err("fmt chunk 截断".into());
            }
            let audio_format = u16::from_le_bytes(bytes[body..body + 2].try_into().expect("2"));
            if audio_format != 1 {
                return Err("仅支持 PCM WAV（虚拟拼接前提）".into());
            }
            channels = u16::from_le_bytes(bytes[body + 2..body + 4].try_into().expect("2"));
            sample_rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into().expect("4"));
            bits = u16::from_le_bytes(bytes[body + 14..body + 16].try_into().expect("2"));
            have_fmt = true;
        } else if id == b"data" {
            if !have_fmt {
                return Err("data chunk 先于 fmt 出现".into());
            }
            return Ok(WavInfo {
                channels,
                sample_rate,
                bits_per_sample: bits,
                data_offset: body as u64,
                data_len: size,
            });
        }
        // RIFF chunk 字对齐：奇数长度补 1 pad 字节。
        pos = body + size as usize + (size as usize & 1);
    }
    Err("未找到 data chunk".into())
}

/// 合成标准 44 字节 PCM WAV 头。
///
/// 长度字段为 32 位：多分片之和理论上可超 u32（滚动切分正是为此存在）→ 饱和到
/// `u32::MAX`（解码器以实际流长为准；该越界场景极罕见，约 >37 小时）。
#[must_use]
pub fn synthesize_wav_header(channels: u16, sample_rate: u32, bits: u16, data_len: u64) -> Vec<u8> {
    let block_align = channels * (bits / 8);
    let byte_rate = sample_rate * u32::from(block_align);
    let riff_len = u32::try_from(36 + data_len).unwrap_or(u32::MAX);
    let data_len32 = u32::try_from(data_len).unwrap_or(u32::MAX);
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&riff_len.to_le_bytes());
    h.extend_from_slice(b"WAVE");
    h.extend_from_slice(b"fmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes()); // PCM
    h.extend_from_slice(&channels.to_le_bytes());
    h.extend_from_slice(&sample_rate.to_le_bytes());
    h.extend_from_slice(&byte_rate.to_le_bytes());
    h.extend_from_slice(&block_align.to_le_bytes());
    h.extend_from_slice(&bits.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&data_len32.to_le_bytes());
    h
}

/// 读取并解析单个分片的 WAV 头（只读文件首 4KB）。
fn read_wav_info(path: &Path) -> Result<WavInfo, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| format!("打开分片失败：{e}"))?;
    let mut head = [0u8; 4096];
    let mut filled = 0usize;
    loop {
        let n = file
            .read(&mut head[filled..])
            .map_err(|e| format!("读取分片头失败：{e}"))?;
        if n == 0 {
            break;
        }
        filled += n;
        if filled == head.len() {
            break;
        }
    }
    parse_wav_header(&head[..filled])
}

/// 由有序分片路径构建拼接布局：解析各头 + fmt 一致性校验 + 合成头。
///
/// # Errors
/// 分片为空、不可读、非 PCM WAV、或 fmt（声道/采样率/位深）不一致时返回错误。
/// 生成一个每会话 token（两个 JobId 拼接，足够长且不可猜）。
fn new_session_token() -> String {
    format!("{}{}", JobId::new(), JobId::new())
}

pub fn build_composite_layout(parts: &[PathBuf]) -> Result<CompositeLayout, String> {
    if parts.is_empty() {
        return Err("分片列表为空".into());
    }
    let mut infos = Vec::with_capacity(parts.len());
    for path in parts {
        infos.push((path.clone(), read_wav_info(path)?));
    }
    let first = infos[0].1;
    if infos.iter().any(|(_, i)| {
        i.channels != first.channels
            || i.sample_rate != first.sample_rate
            || i.bits_per_sample != first.bits_per_sample
    }) {
        return Err("分片 fmt 不一致，无法虚拟拼接".into());
    }
    let total: u64 = infos.iter().map(|(_, i)| i.data_len).sum();
    let header = synthesize_wav_header(
        first.channels,
        first.sample_rate,
        first.bits_per_sample,
        total,
    );
    Ok(CompositeLayout {
        header,
        parts: infos
            .into_iter()
            .map(|(path, i)| CompositePart {
                path,
                data_offset: i.data_offset,
                data_len: i.data_len,
            })
            .collect(),
    })
}

/// 把虚拟时间轴上的字节 Range 映射为「合成头/分片文件」读取切片序列（纯函数）。
///
/// 调用方保证 `range` 已被 [`parse_range`] 按 `total_len` 约束（不会越界）。
#[must_use]
pub fn map_composite_range(layout: &CompositeLayout, range: ByteRange) -> Vec<CompositeSlice> {
    let mut slices = Vec::new();
    let mut virt = range.start;
    let mut remaining = range.end - range.start + 1;
    let header_len = layout.header.len() as u64;

    if virt < header_len && remaining > 0 {
        let take = remaining.min(header_len - virt);
        slices.push(CompositeSlice::Header {
            offset: virt as usize,
            len: take as usize,
        });
        virt += take;
        remaining -= take;
    }

    let mut region_start = header_len;
    for (idx, part) in layout.parts.iter().enumerate() {
        if remaining == 0 {
            break;
        }
        let region_end = region_start + part.data_len;
        if virt < region_end {
            let inner = virt - region_start;
            let take = remaining.min(part.data_len - inner);
            slices.push(CompositeSlice::Part {
                idx,
                file_offset: part.data_offset + inner,
                len: take,
            });
            virt += take;
            remaining -= take;
        }
        region_start = region_end;
    }
    slices
}

impl MediaServer {
    /// 启动本地媒体 Range 服务（127.0.0.1）。启动时不绑定任何 Vault 根；待 `set_root` 设置后服务文件。
    pub fn spawn() -> io::Result<Self> {
        let token: Arc<RwLock<String>> = Arc::new(RwLock::new(new_session_token()));
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let listener = TcpListener::from_std(listener)?;
        let root: Arc<RwLock<Option<PathBuf>>> = Arc::new(RwLock::new(None));
        let composites: Arc<RwLock<HashMap<String, CompositeLayout>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let server = Self {
            base_url: format!("http://127.0.0.1:{port}"),
            token: Arc::clone(&token),
            root: Arc::clone(&root),
            composites: Arc::clone(&composites),
        };
        tauri::async_runtime::spawn(async move {
            run(listener, root, composites, token).await;
        });
        Ok(server)
    }

    /// 设置/切换当前 Vault 根（select_vault / create_vault_root 后调用）。
    /// 同时清空拼接注册表（旧 Vault 的媒体句柄不得跨 Vault 存活）。
    /// 轮换会话 token（切库收尾，TDD-12 §4 ⑤）：旧库遗留在前端的媒体 URL 立即失效——
    /// 「一个服务实例不得同时可达两个库」这条边界，靠换根 + 换 token 两手共同兜住。
    ///
    /// 单库时代这条由"改设置 + 重启"自动满足（重启即新 token）；切库不再重启后必须显式做。
    pub fn rotate_token(&self) {
        if let Ok(mut slot) = self.token.write() {
            *slot = new_session_token();
        }
    }

    pub fn set_root(&self, root: PathBuf) {
        if let Ok(mut slot) = self.root.write() {
            *slot = Some(root);
        }
        if let Ok(mut map) = self.composites.write() {
            map.clear();
        }
    }

    /// 注册多分片 WAV 虚拟拼接资源，返回 composite_id。
    ///
    /// 路径须已由调用方经 [`resolve_vault_path`] 校验（防越界）；本方法解析各分片
    /// RIFF 头并校验 fmt 一致。注册后经 `GET /stream?composite=<id>&token=…`
    /// 以单一连续 WAV 提供 Range 流式。
    ///
    /// # Errors
    /// 分片不可读 / 非 PCM WAV / fmt 不一致时返回错误（调用方可降级单分片直链）。
    pub fn register_composite(&self, parts: Vec<PathBuf>) -> Result<String, String> {
        let layout = build_composite_layout(&parts)?;
        let id = JobId::new().to_string();
        self.composites
            .write()
            .map_err(|_| "composite 注册表锁中毒".to_string())?
            .insert(id.clone(), layout);
        Ok(id)
    }

    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// 当前会话 token（快照——轮换后旧值立即失效，故调用方不缓存）。
    #[must_use]
    pub fn token(&self) -> String {
        self.token.read().map(|t| t.clone()).unwrap_or_default()
    }
}

async fn run(
    listener: TcpListener,
    root: Arc<RwLock<Option<PathBuf>>>,
    composites: Arc<RwLock<HashMap<String, CompositeLayout>>>,
    token: Arc<RwLock<String>>,
) {
    loop {
        match listener.accept().await {
            Ok((stream, addr)) if addr.ip().is_loopback() => {
                // 每连接读取当前 Vault 根快照：未绑定 / canonicalize 失败 → 503（服务暂不可用）。
                let current = root
                    .read()
                    .ok()
                    .and_then(|slot| slot.clone())
                    .and_then(|path| path.canonicalize().ok());
                let token = token.clone();
                let composites = Arc::clone(&composites);
                tauri::async_runtime::spawn(async move {
                    match current {
                        Some(root) => {
                            // 每连接读当前 token（与 root 同构）：切库轮换后旧 URL 即刻失效。
                            let current_token = token.read().map(|t| t.clone()).unwrap_or_default();
                            let _ =
                                handle_connection(stream, &root, &composites, &current_token).await;
                        }
                        None => {
                            let mut stream = stream;
                            let _ = write_status(&mut stream, 503, "Service Unavailable", &[], &[])
                                .await;
                        }
                    }
                });
            }
            Ok((mut stream, _)) => {
                let _ = write_status(&mut stream, 403, "Forbidden", &[], &[]).await;
            }
            Err(e) => {
                eprintln!("[media] 本地媒体服务 accept 失败：{e}");
                break;
            }
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    root: &Path,
    composites: &RwLock<HashMap<String, CompositeLayout>>,
    token: &str,
) -> io::Result<()> {
    let Some(request) = read_request(&mut stream).await? else {
        return Ok(());
    };
    let origin = request.headers.get("origin").map(String::as_str);
    if !origin.is_some_and(is_allowed_origin) {
        // 媒体元素若没设 `crossorigin`，请求里根本没有 Origin 头——这是最常见的 403 来源。
        eprintln!(
            "[media] 403 Origin 校验未过：{}",
            if origin.is_some() {
                "来源不在白名单"
            } else {
                "请求未带 Origin 头（媒体元素缺 crossorigin）"
            }
        );
        return write_status(&mut stream, 403, "Forbidden", &[], &[]).await;
    }
    let cors = cors_headers(origin.expect("origin checked"));

    if request.method == "OPTIONS" {
        return write_status(&mut stream, 204, "No Content", &cors, &[]).await;
    }

    if request.method != "GET" || request.path != "/stream" {
        return write_status(&mut stream, 404, "Not Found", &cors, &[]).await;
    }

    if !is_authorized(&request.headers, &request.query, token) {
        return write_status(&mut stream, 401, "Unauthorized", &cors, &[]).await;
    }

    // 多分片虚拟拼接分支（?composite=<id>，TDD-12 §4）。
    if let Some(id) = query_param(&request.query, "composite").and_then(percent_decode) {
        let layout = composites.read().ok().and_then(|map| map.get(&id).cloned());
        let Some(layout) = layout else {
            return write_status(&mut stream, 404, "Not Found", &cors, &[]).await;
        };
        return serve_composite(&mut stream, &layout, &request, cors).await;
    }

    let Some(path) = query_param(&request.query, "path").and_then(percent_decode) else {
        return write_status(&mut stream, 400, "Bad Request", &cors, &[]).await;
    };

    let file_path = match resolve_vault_path(root, &path) {
        Ok(path) => path,
        Err(reason) => {
            // 只打原因，不打路径（路径里是笔记标题）。
            eprintln!("[media] 403 拒绝流式路径：{reason}");
            return write_status(&mut stream, 403, "Forbidden", &cors, &[]).await;
        }
    };
    let mut file = match tokio::fs::File::open(&file_path).await {
        Ok(file) => file,
        Err(e) => {
            eprintln!("[media] 404 打不开流式文件：{}", e.kind());
            return write_status(&mut stream, 404, "Not Found", &cors, &[]).await;
        }
    };
    let len = file.metadata().await?.len();
    let Some(range) = parse_range(request.headers.get("range").map(String::as_str), len) else {
        return write_status(
            &mut stream,
            416,
            "Range Not Satisfiable",
            &cors,
            &[("Accept-Ranges", "bytes")],
        )
        .await;
    };
    let mime = content_type(&file_path);
    let mut headers = cors;
    headers.extend_from_slice(&[
        ("Accept-Ranges", "bytes".to_string()),
        ("Content-Type", mime.to_string()),
        ("Content-Length", (range.end - range.start + 1).to_string()),
        (
            "Content-Range",
            format!("bytes {}-{}/{}", range.start, range.end, len),
        ),
    ]);
    let status_text = if range.status == 206 {
        "Partial Content"
    } else {
        "OK"
    };
    write_headers(&mut stream, range.status, status_text, &headers).await?;
    file.seek(std::io::SeekFrom::Start(range.start)).await?;
    copy_file_bytes(&mut stream, &mut file, range.end - range.start + 1).await
}

/// 拼接资源响应：按 Range 映射逐切片写出（合成头来自内存，分片来自文件 seek 读取）。
async fn serve_composite(
    stream: &mut TcpStream,
    layout: &CompositeLayout,
    request: &ParsedRequest,
    cors: Vec<(&'static str, String)>,
) -> io::Result<()> {
    let len = layout.total_len();
    let Some(range) = parse_range(request.headers.get("range").map(String::as_str), len) else {
        return write_status(
            stream,
            416,
            "Range Not Satisfiable",
            &cors,
            &[("Accept-Ranges", "bytes")],
        )
        .await;
    };
    let mut headers = cors;
    headers.extend_from_slice(&[
        ("Accept-Ranges", "bytes".to_string()),
        ("Content-Type", "audio/wav".to_string()),
        ("Content-Length", (range.end - range.start + 1).to_string()),
        (
            "Content-Range",
            format!("bytes {}-{}/{}", range.start, range.end, len),
        ),
    ]);
    let status_text = if range.status == 206 {
        "Partial Content"
    } else {
        "OK"
    };
    write_headers(stream, range.status, status_text, &headers).await?;

    for slice in map_composite_range(layout, range) {
        match slice {
            CompositeSlice::Header { offset, len } => {
                stream
                    .write_all(&layout.header[offset..offset + len])
                    .await?;
            }
            CompositeSlice::Part {
                idx,
                file_offset,
                len,
            } => {
                let mut file = tokio::fs::File::open(&layout.parts[idx].path).await?;
                file.seek(std::io::SeekFrom::Start(file_offset)).await?;
                copy_file_bytes(stream, &mut file, len).await?;
            }
        }
    }
    Ok(())
}

/// 从文件当前位置向流复制至多 `remaining` 字节（分块，不整段载入内存）。
async fn copy_file_bytes(
    stream: &mut TcpStream,
    file: &mut tokio::fs::File,
    mut remaining: u64,
) -> io::Result<()> {
    let mut buf = [0_u8; 16 * 1024];
    while remaining > 0 {
        let max = remaining.min(buf.len() as u64) as usize;
        let read = file.read(&mut buf[..max]).await?;
        if read == 0 {
            break;
        }
        stream.write_all(&buf[..read]).await?;
        remaining -= read as u64;
    }
    Ok(())
}

struct ParsedRequest {
    method: String,
    path: String,
    query: String,
    headers: HashMap<String, String>,
}

async fn read_request(stream: &mut TcpStream) -> io::Result<Option<ParsedRequest>> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..read]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 16 * 1024 {
            return Ok(None);
        }
    }

    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.split("\r\n");
    let Some(first) = lines.next() else {
        return Ok(None);
    };
    let mut first_parts = first.split_whitespace();
    let Some(method) = first_parts.next() else {
        return Ok(None);
    };
    let Some(target) = first_parts.next() else {
        return Ok(None);
    };
    let (path, query) = target
        .split_once('?')
        .map_or((target, ""), |(path, query)| (path, query));

    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    Ok(Some(ParsedRequest {
        method: method.to_string(),
        path: path.to_string(),
        query: query.to_string(),
        headers,
    }))
}

pub fn resolve_vault_path(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel_path = Path::new(rel);
    for comp in rel_path.components() {
        match comp {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("path escapes vault".into());
            }
            _ => {}
        }
    }
    let root = root
        .canonicalize()
        .map_err(|e| format!("vault root is not canonical: {e}"))?;
    let target = root.join(rel_path);
    let target = target
        .canonicalize()
        .map_err(|e| format!("media path is not canonical: {e}"))?;
    if !target.starts_with(&root) || !target.is_file() {
        return Err("path escapes vault".into());
    }
    Ok(target)
}

/// 解析**笔记相对**的附件路径（`![](assets/x.png)` / `![](../Att/x.png)` 等）。
///
/// 与 [`resolve_vault_path`] 的关键区别：**允许库内 `..`**——附件位置策略（PRD-06-012
/// 四模式：仓库根 / 同级 / 子目录 / 指定目录）会产出形如 `../Attachments/x.png` 的合法
/// 笔记相对链接，若照搬「禁一切 `..`」规则则这些模式的图片永远解析失败。安全性仍由
/// **规范化后 `starts_with(root)`** 兜底（`../../etc/passwd` 规范化后落在根外即拒），
/// 越权面不扩大。目标不存在（断链）时 `canonicalize` 失败 → 返回错误，前端呈占位。
pub fn resolve_note_relative_asset(
    root: &Path,
    note_rel: &str,
    asset_rel: &str,
) -> Result<PathBuf, String> {
    let note_dir = Path::new(note_rel)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let root = root
        .canonicalize()
        .map_err(|e| format!("vault root is not canonical: {e}"))?;
    let target = root
        .join(note_dir)
        .join(asset_rel)
        .canonicalize()
        .map_err(|e| format!("asset path is not canonical: {e}"))?;
    if !target.starts_with(&root) || !target.is_file() {
        return Err("path escapes vault".into());
    }
    Ok(target)
}

pub fn parse_range(range: Option<&str>, len: u64) -> Option<ByteRange> {
    if len == 0 {
        return None;
    }
    let Some(range) = range else {
        return Some(ByteRange {
            start: 0,
            end: len - 1,
            status: 200,
        });
    };
    let spec = range.strip_prefix("bytes=")?;
    let (start, end) = spec.split_once('-')?;
    if start.is_empty() {
        let suffix: u64 = end.parse().ok()?;
        if suffix == 0 {
            return None;
        }
        let start = len.saturating_sub(suffix);
        return Some(ByteRange {
            start,
            end: len - 1,
            status: 206,
        });
    }

    let start: u64 = start.parse().ok()?;
    if start >= len {
        return None;
    }
    let end = if end.is_empty() {
        len - 1
    } else {
        end.parse::<u64>().ok()?.min(len - 1)
    };
    if end < start {
        return None;
    }
    Some(ByteRange {
        start,
        end,
        status: 206,
    })
}

pub fn is_allowed_origin(origin: &str) -> bool {
    origin == "tauri://localhost"
        || origin == "http://tauri.localhost"
        || origin == "https://tauri.localhost"
        || origin.starts_with("http://localhost:")
        || origin.starts_with("http://127.0.0.1:")
}

/// 鉴权（TDD-12 §4）：fetch 调用方走 `Authorization: Bearer`；**`<audio>`/`<video>` 媒体元素
/// 无法设置请求头**，故额外接受同一 per-session token 经 `?token=` 查询参——风险有界：仅
/// 127.0.0.1 绑定 + Origin/CORS 校验（调用方在 `handle_connection` 已先校验 Origin）+ 每会话轮换，
/// 桌面 webview 无外发 referrer 泄漏面。
#[must_use]
pub fn is_authorized(headers: &HashMap<String, String>, query: &str, token: &str) -> bool {
    has_bearer(headers, token) || query_param(query, "token") == Some(token)
}

pub fn has_bearer(headers: &HashMap<String, String>, token: &str) -> bool {
    headers
        .get("authorization")
        .is_some_and(|value| value == &format!("Bearer {token}"))
}

fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key == name).then_some(value)
    })
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                index += 3;
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// 受管媒体的 MIME（`<audio>`/`<video>` 据此选解码器；未知类型诚实回落二进制流）。
/// 覆盖范围与可播放媒体扩展名单一来源（`ipc::playback::{AUDIO,VIDEO}_MEDIA_EXTS`）对齐。
fn content_type(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("mp3") => "audio/mpeg",
        Some("m4a" | "aac") => "audio/mp4",
        Some("ogg" | "opus") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("flac") => "audio/flac",
        // 视频容器（PRD-04-058 内嵌视频回放）：mp4 家族 + QuickTime。
        Some("mp4" | "m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        _ => "application/octet-stream",
    }
}

fn cors_headers(origin: &str) -> Vec<(&'static str, String)> {
    vec![
        ("Access-Control-Allow-Origin", origin.to_string()),
        (
            "Access-Control-Allow-Headers",
            "Authorization, Range".into(),
        ),
        ("Access-Control-Allow-Methods", "GET, OPTIONS".into()),
        ("Vary", "Origin".into()),
    ]
}

async fn write_status(
    stream: &mut TcpStream,
    code: u16,
    text: &str,
    dynamic_headers: &[(&'static str, String)],
    static_headers: &[(&'static str, &'static str)],
) -> io::Result<()> {
    let mut headers = dynamic_headers.to_vec();
    headers.push(("Content-Length", "0".into()));
    for (key, value) in static_headers {
        headers.push((key, (*value).into()));
    }
    write_headers(stream, code, text, &headers).await
}

async fn write_headers(
    stream: &mut TcpStream,
    code: u16,
    text: &str,
    headers: &[(&'static str, String)],
) -> io::Result<()> {
    let mut response = format!("HTTP/1.1 {code} {text}\r\nConnection: close\r\n");
    for (key, value) in headers {
        response.push_str(key);
        response.push_str(": ");
        response.push_str(value);
        response.push_str("\r\n");
    }
    response.push_str("\r\n");
    stream.write_all(response.as_bytes()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_parser_bounds_requests() {
        assert_eq!(
            parse_range(Some("bytes=2-5"), 10),
            Some(ByteRange {
                start: 2,
                end: 5,
                status: 206,
            })
        );
        assert_eq!(
            parse_range(Some("bytes=8-99"), 10),
            Some(ByteRange {
                start: 8,
                end: 9,
                status: 206,
            })
        );
        assert_eq!(parse_range(Some("bytes=99-100"), 10), None);
        assert_eq!(
            parse_range(None, 10),
            Some(ByteRange {
                start: 0,
                end: 9,
                status: 200,
            })
        );
    }

    #[test]
    fn auth_and_origin_reject_external_callers() {
        let mut headers = HashMap::new();
        headers.insert("authorization".into(), "Bearer secret".into());
        assert!(has_bearer(&headers, "secret"));
        assert!(!has_bearer(&headers, "wrong"));
        assert!(is_allowed_origin("http://localhost:5173"));
        assert!(is_allowed_origin("tauri://localhost"));
        assert!(!is_allowed_origin("https://example.com"));
    }

    #[test]
    fn unicode_and_space_paths_decode_and_resolve() {
        // 速记留下的录音文件名带空格、中文与全角标点（真机 2026-09-06）：前端
        // `URLSearchParams` 把空格编成 `+`、其余按 UTF-8 百分号编码，这里要能原样解回并落到文件。
        let dir = tempfile::tempdir().unwrap();
        let rel = "QuickNotes/assets/语音速记 2026-09-06 1425 今天是中午，我想吃一个大西瓜。晚.wav";
        std::fs::create_dir_all(dir.path().join("QuickNotes/assets")).unwrap();
        std::fs::write(dir.path().join(rel), b"RIFF").unwrap();
        let encoded: String = rel
            .bytes()
            .map(|b| match b {
                b' ' => "+".to_owned(),
                b'/' => "%2F".to_owned(),
                b if b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_' => {
                    (b as char).to_string()
                }
                b => format!("%{b:02X}"),
            })
            .collect();
        let query = format!("path={encoded}&token=t");
        let decoded = query_param(&query, "path")
            .and_then(percent_decode)
            .unwrap();
        assert_eq!(decoded, rel);
        assert!(resolve_vault_path(dir.path(), &decoded).is_ok());
    }

    #[test]
    fn is_authorized_accepts_bearer_or_query_token() {
        // M2：`<audio>` 无法设头 → 经 ?token= 鉴权（与 Bearer 同一 token）。
        let empty = HashMap::new();
        assert!(
            is_authorized(&empty, "path=Recordings%2Fa.wav&token=secret", "secret"),
            "查询 token 命中应放行"
        );
        assert!(
            !is_authorized(&empty, "path=Recordings%2Fa.wav&token=wrong", "secret"),
            "错误查询 token 应拒绝"
        );
        assert!(
            !is_authorized(&empty, "path=a.wav", "secret"),
            "无 token 应拒绝"
        );
        // Bearer 头路径仍有效。
        let mut headers = HashMap::new();
        headers.insert("authorization".into(), "Bearer secret".into());
        assert!(is_authorized(&headers, "path=a.wav", "secret"));
    }

    #[test]
    fn vault_path_resolution_rejects_escape_and_symlink() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("media")).unwrap();
        std::fs::write(dir.path().join("media/a.wav"), b"123").unwrap();
        assert!(resolve_vault_path(dir.path(), "media/a.wav").is_ok());
        assert!(resolve_vault_path(dir.path(), "../a.wav").is_err());
        assert!(resolve_vault_path(dir.path(), "/tmp/a.wav").is_err());

        #[cfg(unix)]
        {
            let outside = tempfile::NamedTempFile::new().unwrap();
            std::os::unix::fs::symlink(outside.path(), dir.path().join("media/link.wav")).unwrap();
            assert!(resolve_vault_path(dir.path(), "media/link.wav").is_err());
        }
    }

    /// 合成头 → 解析往返一致（含 data_offset=44 规范布局）。
    #[test]
    fn wav_header_synthesize_parse_round_trip() {
        let header = synthesize_wav_header(1, 16_000, 16, 32_000);
        assert_eq!(header.len(), 44);
        let info = parse_wav_header(&header).expect("解析合成头");
        assert_eq!(
            info,
            WavInfo {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                data_offset: 44,
                data_len: 32_000,
            }
        );
    }

    /// 解析器跳过陌生 chunk（如 LIST）并按奇数长度 pad 对齐。
    #[test]
    fn wav_header_parser_walks_unknown_chunks() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&0u32.to_le_bytes()); // RIFF 长度字段不参与遍历
        bytes.extend_from_slice(b"WAVE");
        // 奇数长度陌生 chunk（5 字节 → 1 字节 pad）
        bytes.extend_from_slice(b"LIST");
        bytes.extend_from_slice(&5u32.to_le_bytes());
        bytes.extend_from_slice(&[0xAA; 5]);
        bytes.push(0); // pad
                       // 规范 fmt + data
        let canon = synthesize_wav_header(1, 16_000, 16, 8);
        bytes.extend_from_slice(&canon[12..]); // 跳过 RIFF/WAVE 前缀，复用 fmt+data 段
        let info = parse_wav_header(&bytes).expect("应跳过 LIST 后找到 data");
        assert_eq!(info.channels, 1);
        assert_eq!(info.data_len, 8);
    }

    #[test]
    fn wav_header_parser_rejects_non_pcm_and_garbage() {
        assert!(parse_wav_header(b"not a wav").is_err());
        // audio_format=3 (IEEE float) 应被拒绝
        let mut header = synthesize_wav_header(1, 16_000, 16, 8);
        header[20] = 3;
        assert!(parse_wav_header(&header).is_err());
    }

    /// 用合成头 + 样本字节构造真实分片文件，验证布局构建与 fmt 一致性校验。
    fn write_fixture_wav(path: &Path, sample_rate: u32, data: &[u8]) {
        let mut bytes = synthesize_wav_header(1, sample_rate, 16, data.len() as u64);
        bytes.extend_from_slice(data);
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn composite_layout_builds_and_rejects_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let p0 = dir.path().join("s-p00.wav");
        let p1 = dir.path().join("s-p01.wav");
        write_fixture_wav(&p0, 16_000, &[1u8; 10]);
        write_fixture_wav(&p1, 16_000, &[2u8; 6]);

        let layout = build_composite_layout(&[p0.clone(), p1.clone()]).expect("一致 fmt 应成功");
        assert_eq!(layout.total_len(), 44 + 16);
        assert_eq!(layout.parts.len(), 2);
        assert_eq!(layout.parts[0].data_offset, 44);
        assert_eq!(layout.parts[1].data_len, 6);

        // fmt 不一致（采样率不同）→ 拒绝
        let p2 = dir.path().join("s-p02.wav");
        write_fixture_wav(&p2, 48_000, &[3u8; 4]);
        assert!(build_composite_layout(&[p0, p2]).is_err());
        assert!(build_composite_layout(&[]).is_err(), "空分片列表应拒绝");
    }

    /// Range 映射黄金夹具：跨头/跨片/片内/末片边界。
    #[test]
    fn composite_range_mapping_covers_boundaries() {
        let layout = CompositeLayout {
            header: synthesize_wav_header(1, 16_000, 16, 16),
            parts: vec![
                CompositePart {
                    path: PathBuf::from("p0"),
                    data_offset: 44,
                    data_len: 10,
                },
                CompositePart {
                    path: PathBuf::from("p1"),
                    data_offset: 44,
                    data_len: 6,
                },
            ],
        };
        let total = layout.total_len();
        assert_eq!(total, 60);

        // 整资源（无 Range）：头 + 两片全量
        let full = map_composite_range(
            &layout,
            ByteRange {
                start: 0,
                end: total - 1,
                status: 200,
            },
        );
        assert_eq!(
            full,
            vec![
                CompositeSlice::Header { offset: 0, len: 44 },
                CompositeSlice::Part {
                    idx: 0,
                    file_offset: 44,
                    len: 10,
                },
                CompositeSlice::Part {
                    idx: 1,
                    file_offset: 44,
                    len: 6,
                },
            ]
        );

        // 跨头/片 0 边界：[40, 47] → 头尾 4 字节 + 片 0 前 4 字节
        let straddle_header = map_composite_range(
            &layout,
            ByteRange {
                start: 40,
                end: 47,
                status: 206,
            },
        );
        assert_eq!(
            straddle_header,
            vec![
                CompositeSlice::Header { offset: 40, len: 4 },
                CompositeSlice::Part {
                    idx: 0,
                    file_offset: 44,
                    len: 4,
                },
            ]
        );

        // 跨片 0/片 1 边界：[52, 56] → 片 0 末 2 字节 + 片 1 前 3 字节
        let straddle_parts = map_composite_range(
            &layout,
            ByteRange {
                start: 52,
                end: 56,
                status: 206,
            },
        );
        assert_eq!(
            straddle_parts,
            vec![
                CompositeSlice::Part {
                    idx: 0,
                    file_offset: 44 + 8,
                    len: 2,
                },
                CompositeSlice::Part {
                    idx: 1,
                    file_offset: 44,
                    len: 3,
                },
            ]
        );

        // 片 1 内部单字节（末字节）
        let last_byte = map_composite_range(
            &layout,
            ByteRange {
                start: total - 1,
                end: total - 1,
                status: 206,
            },
        );
        assert_eq!(
            last_byte,
            vec![CompositeSlice::Part {
                idx: 1,
                file_offset: 44 + 5,
                len: 1,
            }]
        );

        // 越界 Range 由 parse_range 拦截（416 路径）
        assert_eq!(parse_range(Some("bytes=60-70"), total), None);
    }

    #[test]
    fn asset_resolution_is_relative_to_note_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("notes/images")).unwrap();
        std::fs::write(dir.path().join("notes/images/a.png"), b"png").unwrap();
        let resolved =
            resolve_note_relative_asset(dir.path(), "notes/today.md", "images/a.png").unwrap();
        assert!(resolved.ends_with("notes/images/a.png"));
        // 断链（目标不存在）→ 错误（canonicalize 失败），前端呈占位。
        assert!(resolve_note_relative_asset(dir.path(), "notes/today.md", "missing.png").is_err());
    }

    #[test]
    fn asset_resolution_allows_in_vault_parent_traversal_but_rejects_escape() {
        // 「指定文件夹 / 仓库根」模式产出 `../` 链接：库内父级引用须能解析（PRD-06-012）。
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Chats")).unwrap();
        std::fs::create_dir_all(dir.path().join("Attachments")).unwrap();
        std::fs::write(dir.path().join("Attachments/pic.png"), b"png").unwrap();
        let resolved =
            resolve_note_relative_asset(dir.path(), "Chats/a.md", "../Attachments/pic.png")
                .unwrap();
        assert!(resolved.ends_with("Attachments/pic.png"));
        // 越出根一律拒绝（即便存在同名文件也不解析到库外）。
        assert!(
            resolve_note_relative_asset(dir.path(), "Chats/a.md", "../../../../etc/hosts").is_err()
        );
    }
}
