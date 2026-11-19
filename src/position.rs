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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenBounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub scale_factor: i32,
}

/// Tính tọa độ đặt cửa sổ theo chế độ (`cursor`, `bottom-right`, `center`),
/// tự lật lên trên / sang trái nếu con trỏ nằm gần mép dưới hoặc mép phải (như khung chat Facebook).
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

/// Gắn cơ chế đặt vị trí chuẩn EWMH (`_NET_MOVERESIZE_WINDOW`) cho Linux Mint Cinnamon (Muffin X11).
pub fn attach_positioning(
    window: &ApplicationWindow,
    mode: PositionMode,
    width: i32,
    height: i32,
) {
    if mode == PositionMode::Default {
        return;
    }

    let cached_target: Rc<Cell<Option<(u32, i32, i32)>>> = Rc::new(Cell::new(None));
    let active_moved = Rc::new(Cell::new(false));

    // Bước 1: Ngay khi cửa sổ map -> flush GTK X11, tính tọa độ 1 lần theo vị trí chuột lúc bấm phím tắt, và di chuyển.
    {
        let cached = cached_target.clone();
        window.connect_map(move |win| {
            if cached.get().is_some() {
                return;
            }
            if let Some(display) = gdk::Display::default() {
                display.sync();
            }
            let Some((xid, tx, ty)) = resolve_x11_target(win, mode, width, height) else {
                return;
            };
            cached.set(Some((xid, tx, ty)));
            apply_x11_position(xid, tx, ty);

            // Dự phòng sau 25ms nếu Window Manager (Muffin) đặt lại vị trí sau MapRequest
            glib::timeout_add_local_once(Duration::from_millis(25), move || {
                apply_x11_position(xid, tx, ty);
            });
        });
    }

    // Bước 2: Ngay khi Muffin hoàn tất cấp focus lần đầu (`is-active = true`) -> gửi _NET_MOVERESIZE_WINDOW 1 lần nữa.
    {
        let cached = cached_target;
        window.connect_is_active_notify(move |win| {
            if !win.is_active() || active_moved.get() {
                return;
            }
            active_moved.set(true);
            let target = cached
                .get()
                .or_else(|| resolve_x11_target(win, mode, width, height));
            if let Some((xid, tx, ty)) = target {
                cached.set(Some((xid, tx, ty)));
                apply_x11_position(xid, tx, ty);
            }
        });
    }
}

fn resolve_x11_target(
    window: &ApplicationWindow,
    mode: PositionMode,
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

    let (tx, ty) = compute_popup_position(mode, cursor_x, cursor_y, win_w, win_h, bounds);
    Some((xid, tx, ty))
}

fn apply_x11_position(xid: u32, x: i32, y: i32) {
    let Ok((conn, screen_num)) = x11rb::connect(None) else {
        return;
    };
    let Some(screen) = conn.setup().roots.get(screen_num) else {
        return;
    };

    // 1. Chuẩn EWMH _NET_MOVERESIZE_WINDOW (giống wmctrl / xdotool — Muffin của Cinnamon luôn tuân thủ):
    //    flags = StaticGravity (10) | X (1<<8) | Y (1<<9) | Source=Pager/User (2<<12)
    if let Ok(cookie) = conn.intern_atom(false, b"_NET_MOVERESIZE_WINDOW") {
        if let Ok(reply) = cookie.reply() {
            let flags: u32 = 10 | (1 << 8) | (1 << 9) | (2 << 12);
            let event = ClientMessageEvent::new(
                32,
                xid,
                reply.atom,
                [flags, x as u32, y as u32, 0, 0],
            );
            let _ = conn.send_event(
                false,
                screen.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            );
        }
    }

    // 2. Dự phòng thêm ConfigureWindow trực tiếp cho các WM tối giản
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
