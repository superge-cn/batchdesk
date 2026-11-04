//! 设计 03 §11.2 的 `UiState.window` 恢复与 §11.3 的离屏判定。
//!
//! 刻意做成**不依赖 Tauri 的纯函数**：窗口生命周期由 `run()` 执行，这里的规则要能在
//! CI 里被断言（真实窗口位置属实机项）。

use kernel::ui_state::WindowState;

/// 一台显示器的可用区域（物理像素，与 Tauri 的 `Monitor::position/size` 同单位）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorRect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

impl MonitorRect {
    fn contains_rect(&self, x: i64, y: i64, width: i64, height: i64) -> bool {
        // §11.3 的措辞是「窗口矩形须在可用显示器内」——按**包含**判定，不按相交：
        // 只露出一条边的窗口恢复回去就是"找不到了"，不如直接居中。
        x >= self.x
            && y >= self.y
            && x + width <= self.x + self.width
            && y + height <= self.y + self.height
    }
}

/// 启动时如何摆放主窗口。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// UiState 里没有可恢复的矩形：什么都不做（首启动与旧版本状态文件都走这条）。
    Nothing,
    /// 只恢复尺寸（存过尺寸但没存坐标）。
    SizeOnly { width: i64, height: i64 },
    /// 矩形仍在某台可用显示器内：原样恢复。
    Restore {
        x: i64,
        y: i64,
        width: i64,
        height: i64,
    },
    /// 离屏或显示器被拔掉：居中主屏，尺寸压进该屏工作区（＝§11.3 的 clamp）。
    Centered {
        x: i64,
        y: i64,
        width: i64,
        height: i64,
    },
}

/// 把保存下来的窗口矩形对当前可用显示器做一次判定（设计 03 §11.3）。
///
/// `monitors[0]` 视为主屏——调用方给的是 `available_monitors()` 的第一个，
/// 与 §11.3 的「居中主屏」一致。显示器列表为空时不摆放：宁可不恢复，
/// 也不要把窗口丢到一个算不出来的位置上。
pub fn decide_placement(saved: Option<&WindowState>, monitors: &[MonitorRect]) -> Placement {
    let Some(saved) = saved else {
        return Placement::Nothing;
    };
    let (Some(width), Some(height)) = (saved.width, saved.height) else {
        return Placement::Nothing;
    };
    let (width, height) = (width as i64, height as i64);
    if width <= 0 || height <= 0 {
        return Placement::Nothing;
    }
    let Some(primary) = monitors.first() else {
        return Placement::Nothing;
    };
    if let (Some(x), Some(y)) = (saved.x, saved.y) {
        let (x, y) = (x as i64, y as i64);
        if monitors
            .iter()
            .any(|monitor| monitor.contains_rect(x, y, width, height))
        {
            return Placement::Restore {
                x,
                y,
                width,
                height,
            };
        }
    } else {
        return Placement::SizeOnly { width, height };
    }
    // 离屏／显示器已不在：居中主屏，并把尺寸压进该屏可用区域。
    let clamped_width = width.min(primary.width.max(1));
    let clamped_height = height.min(primary.height.max(1));
    Placement::Centered {
        x: primary.x + (primary.width - clamped_width) / 2,
        y: primary.y + (primary.height - clamped_height) / 2,
        width: clamped_width,
        height: clamped_height,
    }
}

/// 把当前窗口状态写成 `UiState.window`（关闭时落盘；UiState 本就是可丢失的本机状态）。
pub fn snapshot(
    width: i64,
    height: i64,
    x: i64,
    y: i64,
    maximized: bool,
    monitor_id: Option<String>,
) -> WindowState {
    WindowState {
        width: Some(width as f64),
        height: Some(height as f64),
        x: Some(x as f64),
        y: Some(y as f64),
        maximized: Some(maximized),
        monitor_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(width: f64, height: f64, x: Option<f64>, y: Option<f64>) -> WindowState {
        WindowState {
            width: Some(width),
            height: Some(height),
            x,
            y,
            maximized: None,
            monitor_id: None,
        }
    }

    const MAIN: MonitorRect = MonitorRect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    /// 副屏在主屏右侧偏上，且带负坐标的拼接布局很常见。
    const SECOND: MonitorRect = MonitorRect {
        x: 1920,
        y: -300,
        width: 1280,
        height: 1024,
    };

    #[test]
    fn missing_or_empty_state_leaves_the_window_alone() {
        assert_eq!(decide_placement(None, &[MAIN]), Placement::Nothing);
        // 没有尺寸就无从恢复（只存过 maximized/monitorId 的旧状态文件）。
        let only_flags = WindowState {
            width: None,
            height: None,
            x: None,
            y: None,
            maximized: Some(true),
            monitor_id: Some("x".to_string()),
        };
        assert_eq!(
            decide_placement(Some(&only_flags), &[MAIN]),
            Placement::Nothing
        );
        assert_eq!(
            decide_placement(Some(&saved(1200.0, 800.0, Some(10.0), Some(10.0))), &[]),
            Placement::Nothing,
            "算不出可用显示器时宁可不摆放"
        );
        assert_eq!(
            decide_placement(Some(&saved(0.0, 800.0, Some(10.0), Some(10.0))), &[MAIN]),
            Placement::Nothing,
            "零/负尺寸是被写坏的状态，不能拿去设窗口"
        );
    }

    #[test]
    fn a_rect_still_inside_a_monitor_is_restored_where_it_was() {
        assert_eq!(
            decide_placement(
                Some(&saved(1200.0, 800.0, Some(100.0), Some(60.0))),
                &[MAIN]
            ),
            Placement::Restore {
                x: 100,
                y: 60,
                width: 1200,
                height: 800
            }
        );
    }

    #[test]
    fn a_rect_on_the_secondary_monitor_is_restored_including_negative_offsets() {
        // 副屏原点 y=-300：跨屏负坐标不能被判成"离屏"，否则双屏用户的窗口每次都被拽回主屏。
        assert_eq!(
            decide_placement(
                Some(&saved(1000.0, 700.0, Some(2000.0), Some(-200.0))),
                &[MAIN, SECOND]
            ),
            Placement::Restore {
                x: 2000,
                y: -200,
                width: 1000,
                height: 700
            }
        );
    }

    #[test]
    fn only_full_containment_counts_not_mere_overlap() {
        // 右下角超出主屏一像素：按 §11.3 的"须在显示器内"＝不满足 → 居中，而不是原样恢复。
        let placement = decide_placement(
            Some(&saved(1200.0, 800.0, Some(800.0), Some(300.0))),
            &[MAIN],
        );
        assert!(
            matches!(placement, Placement::Centered { .. }),
            "只露出一角的位置不能算\"在显示器内\"：{placement:?}"
        );
    }

    #[test]
    fn an_off_screen_rect_is_centered_on_the_primary_monitor() {
        // 4K 屏上摆的窗口，回到 1920×1080 的显示器上：既离屏也超大 → 居中并压进工作区。
        assert_eq!(
            decide_placement(
                Some(&saved(3000.0, 1600.0, Some(2500.0), Some(1200.0))),
                &[MAIN]
            ),
            Placement::Centered {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080
            }
        );
        // 尺寸合适但坐标在拔掉的那台屏上：居中主屏（不算超大，故不裁剪）。
        assert_eq!(
            decide_placement(
                Some(&saved(1200.0, 800.0, Some(4000.0), Some(50.0))),
                &[MAIN]
            ),
            Placement::Centered {
                x: 360,
                y: 140,
                width: 1200,
                height: 800
            }
        );
    }

    #[test]
    fn saved_size_without_a_position_restores_size_only() {
        assert_eq!(
            decide_placement(Some(&saved(1280.0, 800.0, None, None)), &[MAIN]),
            Placement::SizeOnly {
                width: 1280,
                height: 800
            }
        );
    }

    #[test]
    fn snapshot_round_trips_through_the_ui_state_patch_shape() {
        let state = snapshot(1280, 800, 40, 30, true, Some("LEN(0x0e6c)".to_string()));
        assert_eq!(state.width, Some(1280.0));
        assert_eq!(state.maximized, Some(true));
        assert_eq!(state.monitor_id.as_deref(), Some("LEN(0x0e6c)"));
    }
}
