use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gdk4_x11::X11Surface;
use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{gdk, glib, ApplicationWindow};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ClientMessageEvent, ConfigureWindowAux, ConnectionExt, EventMask};

use crate::config::PositionMode;

const EDGE_MARGIN: i32 = 12;
const BOTTOM_TASKBAR_PAD: i32 = 52;
const POS_TOLERANCE: i32 = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenBounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub scale_factor: i32,
}

/// Tính tọa độ đặt cửa sổ theo chế độ (`cursor`, `bottom-right`, `center`),
/// tự lật lên trên / sang trái nếu con trỏ nằm gần mép dưới hoặc mép phải (như khung chat Facebook/Zalo).
pub fn compute_popup_position(
    mode: PositionMode,
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

    let max_x = (bounds.right - win_w - margin).max(bounds.left + margin);
    let max_y = (bounds.bottom - win_h - bottom_pad).max(bounds.top + margin);

    let (mut x, mut y) = match mode {
        PositionMode::BottomRight => (max_x, max_y),
        PositionMode::Center => (
            bounds.left + (bounds.right - bounds.left - win_w) / 2,
            bounds.top + (bounds.bottom - bounds.top - bottom_pad - win_h) / 2,
        ),
        PositionMode::Cursor | PositionMode::Default => {
            let mut px = cursor_x + margin;
            let mut py = cursor_y + margin;

            // Gần mép phải -> lật sang trái con trỏ
            if px + win_w > bounds.right - margin {
                px = cursor_x - win_w - margin;
            }

            // Gần mép dưới (ô nhập chat sát đáy màn hình) -> lật lên PHÍA TRÊN con trỏ
            if py + win_h > bounds.bottom - bottom_pad {
                py = cursor_y - win_h - margin;
            }

            (px, py)
        }
    };

    x = x.clamp(bounds.left + margin, max_x);
    y = y.clamp(bounds.top + margin, max_y);
    (x, y)
}

/// Đặt vị trí cửa sổ không bị giật/nháy góc trên trái và chống trượt trên app web nặng (Zalo, Facebook...):
/// 1. Bắt tọa độ chuột ngay thời điểm bấm Win+V.
/// 2. Tạm ẩn độ mờ (`opacity = 0.0`) trong lúc Window Manager (Muffin) đặt cửa sổ.
/// 3. Dịch trước khi map (`realize`), sau đó gửi chuẩn EWMH `_NET_MOVERESIZE_WINDOW` và
///    xác nhận tọa độ thật trên màn hình (`translate_coordinates`) cho tới khi đứng đúng chỗ mới bật `opacity = 1.0`.
pub fn attach_positioning(
    window: &ApplicationWindow,
    mode: PositionMode,
    width: i32,
    height: i32,
) {
    if mode == PositionMode::Default {
        return;
    }

    // Chụp ngay tọa độ đích tại thời điểm bấm Win+V (trước khi người dùng kịp nhúc nhích chuột)
    let Some((target_x, target_y)) = compute_target_from_pointer(mode, width, height) else {
        return;
    };

    // Ẩn độ mờ tạm thời để triệt tiêu hoàn toàn hiện tượng nháy ở góc trên bên trái rồi nhảy sang
    window.set_opacity(0.0);

    // Chốt an toàn tuyệt đối: tối đa 220ms luôn đảm bảo cửa sổ hiện rõ 100%
    {
        let win_weak = window.downgrade();
        glib::timeout_add_local_once(Duration::from_millis(220), move || {
            if let Some(w) = win_weak.upgrade() {
                w.set_opacity(1.0);
            }
        });
    }

    // Bước 1: Trước khi XMapWindow chạy (`realize`), gợi ý sẵn tọa độ (target_x, target_y)
    window.connect_realize(move |win| {
        if let Some(display) = gdk::Display::default() {
            display.sync();
        }
        if let Some(xid) = get_window_xid(win) {
            if let Ok((conn, _)) = x11rb::connect(None) {
                let aux = ConfigureWindowAux::new().x(target_x).y(target_y);
                let _ = conn.configure_window(xid, &aux);
                let _ = conn.flush();
            }
        }
    });

    // Bước 2: Ngay khi map -> giữ 1 kết nối X11 duy nhất, gửi EWMH move & xác nhận tọa độ thật trước khi hiện
    let started = Rc::new(Cell::new(false));
    window.connect_map(move |win| {
        if started.replace(true) {
            return;
        }
        if let Some(display) = gdk::Display::default() {
            display.sync();
        }
        let Some(xid) = get_window_xid(win) else {
            win.set_opacity(1.0);
            return;
        };
        let Ok((conn, screen_num)) = x11rb::connect(None) else {
            win.set_opacity(1.0);
            return;
        };
        let Some(root) = conn.setup().roots.get(screen_num).map(|s| s.root) else {
            win.set_opacity(1.0);
            return;
        };
        let moveresize_atom = conn
            .intern_atom(false, b"_NET_MOVERESIZE_WINDOW")
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.atom);

        send_ewmh_move(&conn, root, moveresize_atom, xid, target_x, target_y);

        let win_weak = win.downgrade();
        let ticks = Cell::new(0u8);

        glib::timeout_add_local(Duration::from_millis(12), move || {
            let Some(w) = win_weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let t = ticks.get().saturating_add(1);
            ticks.set(t);

            let at_target = query_actual_position(&conn, root, xid)
                .map(|(ax, ay)| {
                    (ax - target_x).abs() <= POS_TOLERANCE && (ay - target_y).abs() <= POS_TOLERANCE
                })
                .unwrap_or(false);

            if at_target {
                // Chỉ bật hiển thị khi Muffin đã hoàn tất cấp focus (`is_active`) hoặc sau tối đa ~96ms
                if w.is_active() || t >= 8 {
                    w.set_opacity(1.0);
                    return glib::ControlFlow::Break;
                }
            } else {
                send_ewmh_move(&conn, root, moveresize_atom, xid, target_x, target_y);
            }

            if t >= 16 {
                w.set_opacity(1.0);
                return glib::ControlFlow::Break;
            }

            glib::ControlFlow::Continue
        });
    });
}

fn get_window_xid(window: &ApplicationWindow) -> Option<u32> {
    let surface = window.surface()?;
    let x11_surface = surface.downcast_ref::<X11Surface>()?;
    u32::try_from(x11_surface.xid()).ok()
}

fn compute_target_from_pointer(
    mode: PositionMode,
    win_w: i32,
    win_h: i32,
) -> Option<(i32, i32)> {
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

    Some(compute_popup_position(
        mode, cursor_x, cursor_y, win_w, win_h, bounds,
    ))
}

fn query_actual_position<C: Connection>(conn: &C, root: u32, xid: u32) -> Option<(i32, i32)> {
    let reply = conn
        .translate_coordinates(xid, root, 0, 0)
        .ok()?
        .reply()
        .ok()?;
    Some((i32::from(reply.dst_x), i32::from(reply.dst_y)))
}

fn send_ewmh_move<C: Connection>(
    conn: &C,
    root: u32,
    moveresize_atom: Option<u32>,
    xid: u32,
    x: i32,
    y: i32,
) {
    // Chuẩn EWMH _NET_MOVERESIZE_WINDOW: StaticGravity (10) | X (1<<8) | Y (1<<9) | Source=Pager (2<<12)
    if let Some(atom) = moveresize_atom {
        let flags: u32 = 10 | (1 << 8) | (1 << 9) | (2 << 12);
        let event = ClientMessageEvent::new(32, xid, atom, [flags, x as u32, y as u32, 0, 0]);
        let _ = conn.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        );
    }

    let aux = ConfigureWindowAux::new().x(x).y(y);
    let _ = conn.configure_window(xid, &aux);
    let _ = conn.flush();
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
        let (x, y) = compute_popup_position(PositionMode::Cursor, 600, 300, 372, 480, HD);
        assert_eq!((x, y), (612, 312));
    }

    #[test]
    fn flips_up_and_left_near_bottom_right_chat() {
        let (x, y) = compute_popup_position(PositionMode::Cursor, 1700, 980, 372, 480, HD);
        assert_eq!(x, 1700 - 372 - 12);
        assert_eq!(y, 980 - 480 - 12);
        assert!(x + 372 <= HD.right);
        assert!(y + 480 <= HD.bottom - BOTTOM_TASKBAR_PAD);
    }

    #[test]
    fn supports_bottom_right_and_center_modes() {
        let (bx, by) = compute_popup_position(PositionMode::BottomRight, 100, 100, 372, 480, HD);
        assert_eq!(bx, 1920 - 372 - EDGE_MARGIN);
        assert_eq!(by, 1080 - 480 - BOTTOM_TASKBAR_PAD);

        let (cx, cy) = compute_popup_position(PositionMode::Center, 100, 100, 372, 480, HD);
        assert_eq!(cx, (1920 - 372) / 2);
        assert_eq!(cy, (1080 - BOTTOM_TASKBAR_PAD - 480) / 2);
    }
}
