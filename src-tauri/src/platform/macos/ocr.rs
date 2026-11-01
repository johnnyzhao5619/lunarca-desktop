//! macOS 屏幕截图文字识别（Vision `VNRecognizeTextRequest`，服务 PRD-01-007 截屏翻译）。
//!
//! **本地、离线、零新增依赖**：Vision 是系统内置框架，识别全程不出设备（TDD-12 隐私铁律）；
//! 调用走既有 `objc2::msg_send`（与 permissions.rs 的 `AVCaptureDevice` 同一模式），不引入
//! `objc2-vision` 等新 crate（ADR-005 License 面零扩张）。
//!
//! ⚠ **运行时只能真机验**：headless 仅保证编译；识别质量、语言自动检测需 macOS 桌面实测。

use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyObject;
use objc2::{class, msg_send, sel};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSString, NSURL};
use std::path::Path;

#[link(name = "Vision", kind = "framework")]
extern "C" {}

/// `VNRequestTextRecognitionLevelAccurate`（Vision 头文件枚举值 0；截屏翻译取准确优先，
/// 单张静态图无实时性约束）。
const RECOGNITION_LEVEL_ACCURATE: isize = 0;

/// 识别语言提示（BCP-47，Vision `recognitionLanguages`）。macOS 13+ 由
/// `automaticallyDetectsLanguage` 自动检测、本表仅作提示；旧系统无自动检测时按本表识别。
/// 覆盖弹窗翻译内置模板语种（中/英/日/韩，`luna-core::builtin_presets`）+ 繁体变体。
const RECOGNITION_LANGUAGE_HINTS: &[&str] = &["zh-Hans", "zh-Hant", "en-US", "ja-JP", "ko-KR"];

/// 把 Vision 识别出的行拼为翻译源文本：按观测顺序（Vision 阅读序）逐行换行连接。
///
/// 保留行结构（PRD-01 §5「多段/含格式文本的结构保真」）——行合并/断句交给下游 LLM，
/// 这里不做启发式黏行，避免把表格/列表压平。纯逻辑，可单测。
#[must_use]
pub fn join_recognized_lines(lines: &[String]) -> String {
    lines
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 对磁盘上的截图文件跑 Vision 文字识别，返回拼接后的全文（无文字 → 空串）。
///
/// # Errors
/// 图片不可读 / Vision 请求失败时返回中文错误描述（不含用户内容，日志脱敏纪律）。
pub fn recognize_text_at_path(path: &Path) -> Result<String, String> {
    let path_str = path
        .to_str()
        .ok_or_else(|| "截图路径包含无效字符".to_string())?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path_str));
    let options: Retained<NSDictionary> = NSDictionary::new();

    // SAFETY: Vision 为系统框架；`VNImageRequestHandler initWithURL:options:` 接受文件 URL 与
    // 空选项字典；`VNRecognizeTextRequest init` 无参构造。alloc/init 遵循 objc2 selector family
    // 内存语义（Retained 接管所有权）。
    let (handler, request): (Retained<AnyObject>, Retained<AnyObject>) = unsafe {
        let handler_alloc: Allocated<AnyObject> = msg_send![class!(VNImageRequestHandler), alloc];
        let handler: Retained<AnyObject> = msg_send![
            handler_alloc,
            initWithURL: &*url,
            options: &*options,
        ];
        let request_alloc: Allocated<AnyObject> = msg_send![class!(VNRecognizeTextRequest), alloc];
        let request: Retained<AnyObject> = msg_send![request_alloc, init];
        (handler, request)
    };

    // SAFETY: 三个 setter 均为 VNRecognizeTextRequest 公开属性；`automaticallyDetectsLanguage`
    // 是 macOS 13+ 新增，先 respondsToSelector 探测（老系统回退语言提示表）。
    unsafe {
        let _: () = msg_send![&*request, setRecognitionLevel: RECOGNITION_LEVEL_ACCURATE];
        let _: () = msg_send![&*request, setUsesLanguageCorrection: true];
        let langs: Vec<Retained<NSString>> = RECOGNITION_LANGUAGE_HINTS
            .iter()
            .map(|lang| NSString::from_str(lang))
            .collect();
        let lang_array = NSArray::from_retained_slice(&langs);
        let _: () = msg_send![&*request, setRecognitionLanguages: &*lang_array];
        let auto_detect: bool =
            msg_send![&*request, respondsToSelector: sel!(setAutomaticallyDetectsLanguage:)];
        if auto_detect {
            let _: () = msg_send![&*request, setAutomaticallyDetectsLanguage: true];
        }
    }

    // SAFETY: `performRequests:error:` 同步执行请求数组；`error: _` 由 objc2 转换为 Result。
    let outcome: Result<(), Retained<NSError>> = unsafe {
        let requests = NSArray::from_retained_slice(std::slice::from_ref(&request));
        msg_send![&*handler, performRequests: &*requests, error: _]
    };
    outcome.map_err(|e| format!("Vision 文字识别失败：{}", e.localizedDescription()))?;

    // SAFETY: `results` 返回 NSArray<VNRecognizedTextObservation>；`topCandidates:1` 返回
    // NSArray<VNRecognizedText>；`string` 为其文本属性。均为只读访问。
    let lines: Vec<String> = unsafe {
        let results: Option<Retained<NSArray<AnyObject>>> = msg_send![&*request, results];
        results.map_or_else(Vec::new, |observations| {
            observations
                .to_vec()
                .into_iter()
                .filter_map(|observation| {
                    let candidates: Retained<NSArray<AnyObject>> =
                        msg_send![&*observation, topCandidates: 1_usize];
                    let best: Option<Retained<AnyObject>> = msg_send![&*candidates, firstObject];
                    best.map(|candidate| {
                        let text: Retained<NSString> = msg_send![&*candidate, string];
                        text.to_string()
                    })
                })
                .collect()
        })
    };

    Ok(join_recognized_lines(&lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_keeps_line_structure_and_drops_blank_lines() {
        let lines = vec![
            "  Hello world ".to_string(),
            String::new(),
            "第二行".to_string(),
            "   ".to_string(),
            "third".to_string(),
        ];
        assert_eq!(join_recognized_lines(&lines), "Hello world\n第二行\nthird");
    }

    #[test]
    fn join_empty_observations_yields_empty_string() {
        assert_eq!(join_recognized_lines(&[]), "");
    }
}
