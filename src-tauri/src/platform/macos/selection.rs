//! macOS 选区主动捕获 — **方案 A：AXUIElement 属性直读**（TDD-05 §1.4，首选无感方案）。
//!
//! 通过辅助功能 API 读取前台活动 App 焦点元素的 `kAXSelectedTextAttribute`——**不写剪贴板、
//! 零触碰、零延迟**。失败（无 AX 暴露：部分 Electron/Java 渲染应用）时由调用方退回读剪贴板
//! （双击路径读用户本人那次 `Cmd+C` 的结果，§1.4 收口）。
//!
//! AX 属性名用文档约定的字面量（`"AXFocusedUIElement"`/`"AXSelectedText"`，即 `kAX*Attribute`
//! 常量的值），免去从框架链接 `CFStringRef` 常量。需进程已被授予辅助功能（同 `keytap`）。
//!
//! ⚠ **运行时只能真机验**：headless 仅保证编译。"AX 选区在 Slack/VSCode/浏览器/备忘录是否可读"
//! 是已知跨应用不一致点，须 GUI 实测并维护兼容清单（内部记录）。

use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::geometry::CGRect;
use std::ffi::c_void;
use std::fmt;

use crate::platform::TextInjectionError;
use crate::popup::ScreenRect;

/// 不透明 `AXUIElementRef`（CFType）。
type AXUIElementRef = *mut c_void;
/// `AXError`（`kAXErrorSuccess == 0`）。
type AXError = i32;
const AX_ERROR_SUCCESS: AXError = 0;
const AX_VALUE_CG_RECT_TYPE: i32 = 3;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementCopyParameterizedAttributeValue(
        element: AXUIElementRef,
        parameterized_attribute: CFStringRef,
        parameter: CFTypeRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    fn AXValueGetValue(value: CFTypeRef, value_type: i32, value_ptr: *mut c_void) -> bool;
}

/// 按 Copy/Create 规则持有的 AX/CF 对象。AX 对象跨线程使用由系统 API 管理；Lunarca 只在会话锁保护下
/// 复制/读取这些引用，不把它们暴露到前端或任意业务层。
struct RetainedAxObject(CFType);

impl RetainedAxObject {
    /// # Safety
    /// `reference` 必须是按 Copy/Create 规则返回且非空的 CoreFoundation 对象。
    unsafe fn from_create_rule(reference: CFTypeRef) -> Self {
        Self(CFType::wrap_under_create_rule(reference))
    }

    fn as_cf_ref(&self) -> CFTypeRef {
        self.0.as_CFTypeRef()
    }

    fn as_ax_element_ref(&self) -> AXUIElementRef {
        self.as_cf_ref().cast_mut().cast::<c_void>()
    }
}

impl Clone for RetainedAxObject {
    fn clone(&self) -> Self {
        // SAFETY: `self` owns a live CF object；get-rule 包装会 CFRetain，形成独立 clone。
        unsafe { Self(CFType::wrap_under_get_rule(self.as_cf_ref())) }
    }
}

impl fmt::Debug for RetainedAxObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RetainedAxObject")
            .field(&(self.as_cf_ref() as usize))
            .finish()
    }
}

// AXUIElement/AXValue 是 CoreFoundation 引用；这里仅保存引用并在替换时调用系统 AX API。
// 该封装不提供内部可变 Rust 引用，实际并发由系统对象 + PopupSession 互斥访问约束。
unsafe impl Send for RetainedAxObject {}
unsafe impl Sync for RetainedAxObject {}

/// 触发弹窗那一刻的源 App 选区目标：稳定 AX 元素 + 文本范围。
#[derive(Clone, Debug)]
pub struct MacSelectionSnapshot {
    focused_element: RetainedAxObject,
    selected_range: RetainedAxObject,
}

impl MacSelectionSnapshot {
    #[must_use]
    pub fn selected_text(&self) -> Option<String> {
        // SAFETY: `focused_element` 是捕获时保留的 AXUIElement；Copy 结果由 CFString guard 托管。
        unsafe {
            let selected_ref =
                copy_attribute(self.focused_element.as_ax_element_ref(), "AXSelectedText")?;
            let selected = CFString::wrap_under_create_rule(selected_ref as CFStringRef);
            let text = selected.to_string();
            if text.trim().is_empty() {
                None
            } else {
                Some(text)
            }
        }
    }

    pub fn restore_selected_range(&self) -> Result<(), TextInjectionError> {
        set_attribute_value(
            self.focused_element.as_ax_element_ref(),
            "AXSelectedTextRange",
            self.selected_range.as_cf_ref(),
        )
    }

    pub fn replace_selected_text(&self, text: &str) -> Result<(), TextInjectionError> {
        let _ = self.restore_selected_range();
        let replacement = CFString::new(text);
        set_attribute_value(
            self.focused_element.as_ax_element_ref(),
            "AXSelectedText",
            replacement.as_CFTypeRef(),
        )
    }
}

/// AX 捕获结果：源文本 + 可用于后续替换的稳定目标。
#[derive(Clone, Debug)]
pub struct CapturedSelectionSnapshot {
    pub text: String,
    pub target: MacSelectionSnapshot,
}

fn set_attribute_value(
    element: AXUIElementRef,
    attribute: &str,
    value: CFTypeRef,
) -> Result<(), TextInjectionError> {
    let cf_attr = CFString::new(attribute);
    // SAFETY: `element` 与 `value` 均为调用方持有的有效 AX/CF 引用；函数不接管所有权。
    let err =
        unsafe { AXUIElementSetAttributeValue(element, cf_attr.as_concrete_TypeRef(), value) };
    if err == AX_ERROR_SUCCESS {
        Ok(())
    } else {
        Err(TextInjectionError::AccessibilityUnavailable)
    }
}

/// 读焦点元素的某 AX 属性值（**Copy 规则**：返回值归调用方所有，须释放）。
///
/// # Safety
/// `element` 须为有效的 `AXUIElementRef`。返回的 `CFTypeRef` 由调用方按 create 规则接管释放。
unsafe fn copy_attribute(element: AXUIElementRef, attribute: &str) -> Option<CFTypeRef> {
    let cf_attr = CFString::new(attribute);
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, cf_attr.as_concrete_TypeRef(), &mut value);
    if err == AX_ERROR_SUCCESS && !value.is_null() {
        Some(value)
    } else {
        None
    }
}

/// 读焦点元素的某 AX 参数化属性值（**Copy 规则**：返回值归调用方所有，须释放）。
///
/// # Safety
/// `element` 与 `parameter` 均须为有效 AX/CF 对象。返回的 `CFTypeRef` 由调用方接管释放。
unsafe fn copy_parameterized_attribute(
    element: AXUIElementRef,
    attribute: &str,
    parameter: CFTypeRef,
) -> Option<CFTypeRef> {
    let cf_attr = CFString::new(attribute);
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyParameterizedAttributeValue(
        element,
        cf_attr.as_concrete_TypeRef(),
        parameter,
        &mut value,
    );
    if err == AX_ERROR_SUCCESS && !value.is_null() {
        Some(value)
    } else {
        None
    }
}

/// 读取当前前台 App 焦点元素的**选中文本**（方案 A）。无 AX 暴露 / 无选区时返回 `None`。
///
/// 不接触剪贴板。空白选区视为"无"（返回 `None`），与 [`crate::popup::classify_capture`] 一致。
#[must_use]
pub fn read_selected_text() -> Option<String> {
    // SAFETY: 全程为 AX C API 调用；用 core-foundation 的 create-rule 包装自动释放所有权对象，
    // 不手动 CFRelease（避免双重释放）。AX 调用线程安全（非主线程亦可）。
    unsafe {
        let system_ref = AXUIElementCreateSystemWide();
        if system_ref.is_null() {
            return None;
        }
        // create 规则：包装以在作用域结束自动释放。
        let _system_guard = CFType::wrap_under_create_rule(system_ref.cast::<c_void>());

        let focused_ref = copy_attribute(system_ref, "AXFocusedUIElement")?;
        let _focused_guard = CFType::wrap_under_create_rule(focused_ref);

        let selected_ref = copy_attribute(focused_ref as AXUIElementRef, "AXSelectedText")?;
        // 选中文本为 CFString；create 规则包装自动释放。
        let selected = CFString::wrap_under_create_rule(selected_ref as CFStringRef);
        let text = selected.to_string();

        if text.trim().is_empty() {
            None
        } else {
            Some(text)
        }
    }
}

/// 捕获当前前台 App 焦点元素的源选区文本与稳定 AX range，用于稍后“替换原文”。
#[must_use]
pub fn capture_selected_text_snapshot() -> Option<CapturedSelectionSnapshot> {
    // SAFETY: AX C API 调用；focused/range 按 Copy 规则保留到 `MacSelectionSnapshot`，selected text
    // 临时包装后在本函数内释放。
    unsafe {
        let system_ref = AXUIElementCreateSystemWide();
        if system_ref.is_null() {
            return None;
        }
        let _system_guard = CFType::wrap_under_create_rule(system_ref.cast::<c_void>());

        let focused_ref = copy_attribute(system_ref, "AXFocusedUIElement")?;
        let selected_ref = copy_attribute(focused_ref as AXUIElementRef, "AXSelectedText")?;
        let selected = CFString::wrap_under_create_rule(selected_ref as CFStringRef);
        let text = selected.to_string();
        if text.trim().is_empty() {
            let _focused_guard = CFType::wrap_under_create_rule(focused_ref);
            return None;
        }

        let range_ref = copy_attribute(focused_ref as AXUIElementRef, "AXSelectedTextRange")?;
        Some(CapturedSelectionSnapshot {
            text,
            target: MacSelectionSnapshot {
                focused_element: RetainedAxObject::from_create_rule(focused_ref),
                selected_range: RetainedAxObject::from_create_rule(range_ref),
            },
        })
    }
}

/// 读取当前焦点元素选区的屏幕 bounds（`kAXBoundsForRange`）。
///
/// 返回 `None` 表示目标 App 不暴露该参数化属性或当前没有有效选区。调用方应退回光标定位。
#[must_use]
pub fn read_selected_text_bounds() -> Option<ScreenRect> {
    // SAFETY: 与 `read_selected_text` 相同，AX C API 调用；所有 Copy/Create 结果都交给 CFType
    // create-rule guard 托管释放。
    unsafe {
        let system_ref = AXUIElementCreateSystemWide();
        if system_ref.is_null() {
            return None;
        }
        let _system_guard = CFType::wrap_under_create_rule(system_ref.cast::<c_void>());

        let focused_ref = copy_attribute(system_ref, "AXFocusedUIElement")?;
        let _focused_guard = CFType::wrap_under_create_rule(focused_ref);

        let range_ref = copy_attribute(focused_ref as AXUIElementRef, "AXSelectedTextRange")?;
        let _range_guard = CFType::wrap_under_create_rule(range_ref);

        let bounds_ref = copy_parameterized_attribute(
            focused_ref as AXUIElementRef,
            "AXBoundsForRange",
            range_ref,
        )?;
        let _bounds_guard = CFType::wrap_under_create_rule(bounds_ref);

        let mut rect = CGRect::default();
        if !AXValueGetValue(
            bounds_ref,
            AX_VALUE_CG_RECT_TYPE,
            (&mut rect as *mut CGRect).cast::<c_void>(),
        ) {
            return None;
        }

        if rect.size.width <= 0.0 || rect.size.height <= 0.0 {
            return None;
        }
        Some(ScreenRect::new(
            rect.origin.x,
            rect.origin.y,
            rect.size.width,
            rect.size.height,
        ))
    }
}
