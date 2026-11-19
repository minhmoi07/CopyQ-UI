use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gdk4_x11::X11Surface;
use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{gdk, glib, ApplicationWindow};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConfigureWindowAux, ConnectionExt};

const EDGE_MARGIN: i32 = 12;
const BOTTOM_TASKBAR_PAD: i32 = 48;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenBounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub scale_factor: i32,
}

/// Tính tọa độ đặt cửa sổ ngay gần con trỏ chuột (tự lật lên trên / sang trái
/// nếu con trỏ ở sát đáy hoặc mép phải như khung chat Facebook/Messenger).
pub fn compute_popup_position(
    cursor_x: i32,
    cursor_y: i32,
    win_w_logical: i32,
    win_h_logical: i32,
    bounds: ScreenBounds,
) -> (i32, i32) {
    let sf = bounds.scale_factor.max(1);
    let win_w = win_w_logical * sf;
    let win_h = win_h_logical * sf;
    let margin = EDGE_MARGIN * sf;
    let bottom_pad = BOTTOM_TASKBAR_PAD * sf;

    // Mặc định đặt lệch nhẹ xuống dưới & sang phải con trỏ
    let mut x = cursor_x + margin;
    let mut y = cursor_y + margin;

    // Gần mép phải -> lật sang trái con trỏ
    if x + win_w > bounds.right - margin {
        x = cursor_x - win_w - margin;
    }

    // Gần mép dưới (ví dụ ô nhập chat sát đáy màn hình) -> lật lên PHÍA TRÊN con trỏ
    if y + win_h > bounds.bottom - bottom_pad {
        y = cursor_y - win_h - margin;
    }

    // Đảm bảo cửa sổ luôn nằm trọn trong màn hình hiện tại
    let max_x = (bounds.right - win_w - margin).max(bounds.left + margin);
    let max_y = (bounds.bottom - win_h - bottom_pad).max(bounds.top + margin);
    x = x.clamp(bounds.left + margin, max_x);
    y = y.clamp(bounds.top + margin, max_y);

    (x, y)
}

/// Di chuyển cửa sổ GTK4 (trên X11 / Linux Mint Cinnamon) tới gần con trỏ chuột ngay khi hiện.
pub fn attach_cursor_positioning(window: &ApplicationWindow, width: i32, height: i32) {
    let placed = Rc::new(Cell::new(false));
    window.connect_map(move |win| {
        if placed.replace(true) {
            return;
        }
        let Some((xid, target_x, target_y)) = resolve_x11_target(win, width, height) else {
            return;
        };

        apply_x11_position(xid, target_x, target_y);

        // Một số Window Manager X11 (như Muffin của Cinnamon) có thể tự đặt lại vị trí
        // ngay sau tín hiệu MapRequest -> áp dụng lại 1 lần sau 25ms để đảm bảo đứng đúng chỗ.
        glib::timeout_add_local_once(Duration::from_millis(25), move || {
            apply_x11_position(xid, target_x, target_y);
        });
    });
}

fn resolve_x11_target(
    window: &ApplicationWindow,
    win_w: i32,
    win_h: i32,
) -> Option<(u32, i32, i32)> {
    let surface = window.surface()?;
    let x11_surface = surface.downcast_ref::<X11Surface>()?;
    let xid = u32::try_from(x11_surface.xid()).ok()?;

    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let screen = conn.setup().roots.get(screen_num)?;
    let ptr = conn.query_pointer(screen.root).ok()?.reply().ok()?;

    let cursor_x = i32::from(ptr.root_x);
    let cursor_y = i32::from(ptr.root_y);

    let mut bounds = ScreenBounds {
        left: 0,
        top: 0,
        right: i32::from(screen.width_in_pixels),
        bottom: i32::from(screen.height_in_pixels),
        scale_factor: 1,
    };

    if let Some(display) = gdk::Display::default() {
        let monitors = display.monitors();
        for idx in 0..monitors.n_items() {
            if let Some(obj) = monitors.item(idx) {
                if let Ok(mon) = obj.downcast::<gdk::Monitor>() {
                    let sf = mon.scale_factor().max(1);
                    let geom = mon.geometry();
                    let left = geom.x() * sf;
                    let top = geom.y() * sf;
                    let right = left + geom.width() * sf;
                    let bottom = top + geom.height() * sf;
                    if cursor_x >= left && cursor_x < right && cursor_y >= top && cursor_y < bottom
                    {
                        bounds = ScreenBounds {
                            left,
                            top,
                            right,
                            bottom,
                            scale_factor: sf,
                        };
                        break;
                    }
                }
            }
        }
    }

    let (tx, ty) = compute_popup_position(cursor_x, cursor_y, win_w, win_h, bounds);
    Some((xid, tx, ty))
}

fn apply_x11_position(xid: u32, x: i32, y: i32) {
    if let Ok((conn, _)) = x11rb::connect(None) {
        let aux = ConfigureWindowAux::new().x(x).y(y);
        let _ = conn.configure_window(xid, &aux);
        let _ = conn.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HD: ScreenBounds = ScreenBounds {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
        scale_factor: 1,
    };

    #[test]
    fn places_near_cursor_in_middle() {
        let (x, y) = compute_popup_position(600, 300, 372, 480, HD);
        assert_eq!((x, y), (612, 312));
    }

    #[test]
    fn flips_up_and_left_near_bottom_right_chat() {
        // Giả lập con trỏ ở khung chat Facebook góc dưới bên phải (1700, 980)
        let (x, y) = compute_popup_position(1700, 980, 372, 480, HD);
        assert_eq!(x, 1700 - 372 - 12);
        assert_eq!(y, 980 - 480 - 12);
        assert!(x + 372 <= HD.right);
        assert!(y + 480 <= HD.bottom - BOTTOM_TASKBAR_PAD);
    }

    #[test]
    fn clamps_when_cursor_is_at_corner() {
        let (x, y) = compute_popup_position(1915, 1075, 372, 480, HD);
        assert!(x >= EDGE_MARGIN && x + 372 <= HD.right - EDGE_MARGIN);
        assert!(y >= EDGE_MARGIN && y + 480 <= HD.bottom - BOTTOM_TASKBAR_PAD);
    }
}
