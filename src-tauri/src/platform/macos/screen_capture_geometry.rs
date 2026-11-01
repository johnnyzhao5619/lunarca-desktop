//! ScreenCaptureKit 的单显示器区域裁剪几何基线（S15）。
//!
//! ScreenCaptureKit 的 `sourceRect` 以**所选显示器的逻辑点**为坐标系，而输出尺寸以像素
//! 计。此模块故意不接 Tauri 的全局物理坐标，也不猜测跨屏区域：用户先选定一块显示器，
//! 再在该屏覆盖层中框选；任何越界矩形都必须由调用方要求重新选择。

/// 覆盖层内、相对于已选显示器左上角的逻辑点矩形。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// 已选显示器的逻辑点尺寸与该屏 point-to-pixel scale。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayGeometry {
    pub logical_width: f64,
    pub logical_height: f64,
    pub scale: f64,
}

/// 交给 ScreenCaptureKit 的裁剪计划。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenCapturePlan {
    /// `SCStreamConfiguration.sourceRect`：显示器本地逻辑点矩形。
    pub source_rect: LogicalRect,
    /// `SCStreamConfiguration.width/height`：目标像素尺寸。
    pub pixel_width: u32,
    pub pixel_height: u32,
}

/// 将覆盖层框选转成单显示器 ScreenCaptureKit 裁剪计划。
///
/// # Errors
///
/// 几何数据非有限、scale 非正、矩形为空或越过选定显示器边界时失败。跨屏自动拆分会破坏
/// “一块显示器 → 一个上游过滤器 → 一个坐标系”的 S15 约束，故不在此处实现。
pub fn plan_capture(
    selection: LogicalRect,
    display: DisplayGeometry,
) -> Result<ScreenCapturePlan, &'static str> {
    let values = [
        selection.x,
        selection.y,
        selection.width,
        selection.height,
        display.logical_width,
        display.logical_height,
        display.scale,
    ];
    if values.iter().any(|value| !value.is_finite()) {
        return Err("捕获几何包含非有限数值");
    }
    if display.logical_width <= 0.0 || display.logical_height <= 0.0 || display.scale <= 0.0 {
        return Err("显示器几何无效");
    }
    if selection.width <= 0.0 || selection.height <= 0.0 {
        return Err("捕获区域为空");
    }
    if selection.x < 0.0
        || selection.y < 0.0
        || selection.x + selection.width > display.logical_width
        || selection.y + selection.height > display.logical_height
    {
        return Err("捕获区域越出已选显示器");
    }

    let pixel_width = (selection.width * display.scale).round();
    let pixel_height = (selection.height * display.scale).round();
    if pixel_width < 1.0
        || pixel_height < 1.0
        || pixel_width > u32::MAX as f64
        || pixel_height > u32::MAX as f64
    {
        return Err("捕获输出像素尺寸无效");
    }

    Ok(ScreenCapturePlan {
        source_rect: selection,
        pixel_width: pixel_width as u32,
        pixel_height: pixel_height as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retina_selection_keeps_logical_source_and_doubles_output_pixels() {
        let plan = plan_capture(
            LogicalRect {
                x: 100.0,
                y: 80.0,
                width: 320.0,
                height: 120.0,
            },
            DisplayGeometry {
                logical_width: 1512.0,
                logical_height: 982.0,
                scale: 2.0,
            },
        )
        .unwrap();
        assert_eq!(plan.source_rect.x, 100.0);
        assert_eq!(plan.pixel_width, 640);
        assert_eq!(plan.pixel_height, 240);
    }

    #[test]
    fn non_integer_scale_rounds_only_output_pixels() {
        let plan = plan_capture(
            LogicalRect {
                x: 0.0,
                y: 0.0,
                width: 101.0,
                height: 51.0,
            },
            DisplayGeometry {
                logical_width: 1920.0,
                logical_height: 1080.0,
                scale: 1.5,
            },
        )
        .unwrap();
        assert_eq!(plan.pixel_width, 152);
        assert_eq!(plan.pixel_height, 77);
    }

    #[test]
    fn cross_display_or_offscreen_selection_is_rejected() {
        let result = plan_capture(
            LogicalRect {
                x: 1400.0,
                y: 10.0,
                width: 200.0,
                height: 100.0,
            },
            DisplayGeometry {
                logical_width: 1512.0,
                logical_height: 982.0,
                scale: 2.0,
            },
        );
        assert_eq!(result, Err("捕获区域越出已选显示器"));
    }
}
