use gtk::gdk;
use gtk::prelude::*;
use gtk::ApplicationWindow;

use crate::config::PositionMode;

const EDGE_MARGIN: i32 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkArea {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Tính tọa độ đặt cửa sổ theo chế độ (`cursor`, `bottom-right`, `center`),
/// tự lật lên trên / sang trái nếu con trỏ nằm gần mép dưới hoặc mép phải (như khung chat Facebook/Zalo).
pub fn compute_popup_position(
    mode: PositionMode,
    cursor_x: i32,
    cursor_y: i32,
    win_w: i32,
    win_h: i32,
    area: WorkArea,
) -> (i32, i32) {
    let max_x = (area.right - win_w - EDGE_MARGIN).max(area.left + EDGE_MARGIN);
    let max_y = (area.bottom - win_h - EDGE_MARGIN).max(area.top + EDGE_MARGIN);

    let (mut x, mut y) = match mode {
        PositionMode::BottomRight => (max_x, max_y),
        PositionMode::Center => (
            area.left + (area.right - area.left - win_w) / 2,
            area.top + (area.bottom - area.top - win_h) / 2,
        ),
        PositionMode::Cursor | PositionMode::Default => {
            let mut px = cursor_x + EDGE_MARGIN;
            let mut py = cursor_y + EDGE_MARGIN;

            // Gần mép phải -> lật sang trái con trỏ
            if px + win_w > area.right - EDGE_MARGIN {
                px = cursor_x - win_w - EDGE_MARGIN;
            }

            // Gần mép dưới (ô nhập chat sát đáy màn hình) -> lật lên PHÍA TRÊN con trỏ
            if py + win_h > area.bottom - EDGE_MARGIN {
                py = cursor_y - win_h - EDGE_MARGIN;
            }

            (px, py)
        }
    };

    x = x.clamp(area.left + EDGE_MARGIN, max_x);
    y = y.clamp(area.top + EDGE_MARGIN, max_y);
    (x, y)
}

/// Đặt trực tiếp tọa độ cửa sổ GTK3 TRƯỚC khi gọi `show_all()` bằng `window.move_(x, y)`.
/// Cửa sổ xuất hiện thẳng tại đúng chỗ chuột từ khung hình đầu tiên, tuyệt đối không nháy góc trên trái.
pub fn position_before_show(
    window: &ApplicationWindow,
    mode: PositionMode,
    win_w: i32,
    win_h: i32,
) {
    if mode == PositionMode::Default {
        return;
    }

    let Some(display) = gdk::Display::default() else {
        return;
    };
    let Some(seat) = display.default_seat() else {
        return;
    };
    let Some(pointer) = seat.pointer() else {
        return;
    };

    let (_screen, cursor_x, cursor_y) = pointer.position();

    let monitor = display
        .monitor_at_point(cursor_x, cursor_y)
        .or_else(|| display.primary_monitor())
        .or_else(|| display.monitor(0));

    let Some(mon) = monitor else {
        return;
    };

    // `workarea()` của GTK3 đã tự động trừ chính xác thanh taskbar của Linux Mint Cinnamon
    let rect = mon.workarea();
    let area = WorkArea {
        left: rect.x(),
        top: rect.y(),
        right: rect.x() + rect.width(),
        bottom: rect.y() + rect.height(),
    };

    let (tx, ty) = compute_popup_position(mode, cursor_x, cursor_y, win_w, win_h, area);
    window.set_gravity(gdk::Gravity::Static);
    window.move_(tx, ty);
}

#[cfg(test)]
mod tests {
    use super::*;

    // Giả lập màn hình 1920x1080 trừ thanh taskbar Mint cao 40px dưới đáy -> bottom = 1040
    const HD_WORK: WorkArea = WorkArea {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    };

    #[test]
    fn places_near_cursor_in_middle() {
        let (x, y) = compute_popup_position(PositionMode::Cursor, 600, 300, 372, 480, HD_WORK);
        assert_eq!((x, y), (612, 312));
    }

    #[test]
    fn flips_up_and_left_near_bottom_right_chat() {
        let (x, y) = compute_popup_position(PositionMode::Cursor, 1700, 980, 372, 480, HD_WORK);
        assert_eq!(x, 1700 - 372 - EDGE_MARGIN);
        assert_eq!(y, 980 - 480 - EDGE_MARGIN);
        assert!(x + 372 <= HD_WORK.right - EDGE_MARGIN);
        assert!(y + 480 <= HD_WORK.bottom - EDGE_MARGIN);
    }

    #[test]
    fn supports_bottom_right_and_center_modes() {
        let (bx, by) =
            compute_popup_position(PositionMode::BottomRight, 100, 100, 372, 480, HD_WORK);
        assert_eq!(bx, 1920 - 372 - EDGE_MARGIN);
        assert_eq!(by, 1040 - 480 - EDGE_MARGIN);

        let (cx, cy) = compute_popup_position(PositionMode::Center, 100, 100, 372, 480, HD_WORK);
        assert_eq!(cx, (1920 - 372) / 2);
        assert_eq!(cy, (1040 - 480) / 2);
    }
}
