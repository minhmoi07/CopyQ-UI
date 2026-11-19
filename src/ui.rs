use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use gtk::gdk_pixbuf::{InterpType, Pixbuf, PixbufLoader};
use gtk::prelude::*;
use gtk::{
    cairo, gdk, glib, pango, Align, Application, ApplicationWindow, Button, CssProvider, EventBox,
    IconSize, IconTheme, Image, Justification, Label, ListBox, ListBoxRow, MenuButton, Orientation,
    Overlay, PolicyType, Popover, PositionType, ReliefStyle, ScrolledWindow, SearchEntry,
    SelectionMode, Settings, StyleContext,
};

use crate::config::{AppConfig, ImageFit, Theme, DEFAULT_CSS};
use crate::copyq::{self, ClipboardItem, ItemContent};
use crate::position;

const THUMB_HEIGHT: i32 = 112;

#[derive(Clone, Copy, PartialEq, Eq)]
enum FilterTab {
    All,
    Pinned,
    Images,
}

enum UiItemKind {
    Text {
        display: String,
        search: String,
    },
    Image {
        thumb: Option<Pixbuf>,
        full: Option<Pixbuf>,
    },
}

struct UiItem {
    row: usize,
    pinned: bool,
    kind: UiItemKind,
}

struct AppState {
    config: AppConfig,
    items: Vec<UiItem>,
    visible_indices: Vec<usize>,
    filter: FilterTab,
    query: String,
    loading: bool,
    error_message: Option<String>,
    zoomed_item_index: Option<usize>,
}

#[derive(Clone)]
struct UiHandles {
    window: ApplicationWindow,
    list_box: ListBox,
    vadj: gtk::Adjustment,
    search_entry: SearchEntry,
    theme_btn: Button,
    theme_icon: Image,
    tab_all: Button,
    tab_pinned: Button,
    tab_images: Button,
    zoom_overlay: gtk::Box,
    zoom_image: Image,
    state: Rc<RefCell<AppState>>,
    popover_open_count: Rc<Cell<u32>>,
    load_generation: Rc<Cell<u64>>,
}

pub fn build_ui(app: &Application) {
    // Nếu popup đang mở mà bấm Win+V lần 2 -> đóng luôn (toggle)
    let existing = app.windows();
    if !existing.is_empty() {
        for win in existing {
            win.close();
        }
        return;
    }

    let config = AppConfig::load_or_create();
    load_stylesheet();

    let window = ApplicationWindow::builder()
        .application(app)
        .title("CopyQ-UI")
        .decorated(false)
        .resizable(false)
        .default_width(config.width)
        .default_height(config.height)
        .build();
    window.set_skip_taskbar_hint(true);
    window.set_skip_pager_hint(true);
    window.set_keep_above(true);
    window.style_context().add_class("popup-window");

    // Bật RGBA visual để bo góc cửa sổ mịn trên Linux Mint Cinnamon (không bị viền đen góc vuông)
    if let Some(screen) = gdk::Screen::default() {
        if let Some(visual) = screen.rgba_visual() {
            window.set_visual(Some(&visual));
        }
    }
    window.set_app_paintable(true);
    window.connect_draw(|_, cr| {
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
        cr.set_operator(cairo::Operator::Source);
        let _ = cr.paint();
        cr.set_operator(cairo::Operator::Over);
        glib::Propagation::Proceed
    });

    let popup_container = gtk::Box::new(Orientation::Vertical, 0);
    popup_container.style_context().add_class("popup-container");
    popup_container.set_size_request(config.width, config.height);

    // ===== 1. HEADER: Logo CopyQ (trái) | Tìm kiếm (giữa vừa khít) | Nút Sáng/Tối (phải) =====
    let header_bar = gtk::Box::new(Orientation::Horizontal, 8);
    header_bar.style_context().add_class("header-bar");

    let logo = create_copyq_logo();
    logo.style_context().add_class("copyq-logo");
    logo.set_tooltip_text(Some("CopyQ"));

    let search_entry = SearchEntry::new();
    search_entry.style_context().add_class("search-input");
    search_entry.set_placeholder_text(Some("Tìm kiếm..."));
    search_entry.set_hexpand(true);

    let theme_icon = Image::from_icon_name(Some("weather-clear-symbolic"), IconSize::Button);
    theme_icon.set_pixel_size(16);
    let theme_btn = Button::new();
    theme_btn.style_context().add_class("theme-toggle-btn");
    theme_btn.set_image(Some(&theme_icon));
    theme_btn.set_always_show_image(true);
    theme_btn.set_can_focus(false);

    header_bar.pack_start(&logo, false, false, 0);
    header_bar.pack_start(&search_entry, true, true, 0);
    header_bar.pack_end(&theme_btn, false, false, 0);

    // ===== 2. THANH LỌC (Tất cả / Đã ghim / Ảnh) =====
    let filter_bar = gtk::Box::new(Orientation::Horizontal, 4);
    filter_bar.style_context().add_class("filter-bar");

    let tab_all = make_filter_button("Tất cả");
    let tab_pinned = make_filter_button("Đã ghim");
    let tab_images = make_filter_button("Ảnh");

    filter_bar.pack_start(&tab_all, false, false, 0);
    filter_bar.pack_start(&tab_pinned, false, false, 0);
    filter_bar.pack_start(&tab_images, false, false, 0);

    // ===== 3. DANH SÁCH THẺ CỐ ĐỊNH KHUNG CHIỀU CAO =====
    let list_box = ListBox::new();
    list_box.style_context().add_class("clip-list");
    list_box.set_selection_mode(SelectionMode::Single);
    list_box.set_activate_on_single_click(true);

    let vadj = gtk::Adjustment::new(0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    let scrolled = ScrolledWindow::new(None::<&gtk::Adjustment>, Some(&vadj));
    scrolled.style_context().add_class("clip-scroll");
    scrolled.set_policy(PolicyType::Never, PolicyType::Automatic);
    scrolled.set_vexpand(true);
    scrolled.set_hexpand(true);
    scrolled.set_propagate_natural_height(false);
    scrolled.set_propagate_natural_width(false);
    scrolled.add(&list_box);

    // ===== 4. THANH GỢI Ý PHÍM TẮT =====
    let footer_bar = gtk::Box::new(Orientation::Horizontal, 8);
    footer_bar.style_context().add_class("footer-bar");

    let hint_left = Label::new(Some("↑↓ Chọn · Enter Dán · Shift+Enter Chữ thuần"));
    hint_left.set_xalign(0.0);

    let hint_right = Label::new(Some("Esc Đóng"));
    hint_right.set_xalign(1.0);

    footer_bar.pack_start(&hint_left, true, true, 0);
    footer_bar.pack_end(&hint_right, false, false, 0);

    // Cho phép nắm kéo di chuyển cửa sổ từ thanh tiêu đề, dải lọc hoặc chân trang
    let header_drag = make_draggable(&header_bar, &window);
    let filter_drag = make_draggable(&filter_bar, &window);
    let footer_drag = make_draggable(&footer_bar, &window);

    let main_box = gtk::Box::new(Orientation::Vertical, 0);
    main_box.set_vexpand(true);
    main_box.set_hexpand(true);
    main_box.pack_start(&header_drag, false, false, 0);
    main_box.pack_start(&filter_drag, false, false, 0);
    main_box.pack_start(&scrolled, true, true, 0);
    main_box.pack_end(&footer_drag, false, false, 0);

    // ===== 5. LỚP PHỦ PHÓNG TO ẢNH (ZOOM OVERLAY) =====
    let (zoom_overlay, zoom_image, zoom_close_btn, zoom_paste_btn) = build_zoom_overlay();

    let root_overlay = Overlay::new();
    root_overlay.set_vexpand(true);
    root_overlay.set_hexpand(true);
    root_overlay.add(&main_box);
    root_overlay.add_overlay(&zoom_overlay);

    popup_container.pack_start(&root_overlay, true, true, 0);
    window.add(&popup_container);

    let pos_mode = config.position;
    let win_w = config.width;
    let win_h = config.height;

    let state = Rc::new(RefCell::new(AppState {
        config,
        items: Vec::new(),
        visible_indices: Vec::new(),
        filter: FilterTab::All,
        query: String::new(),
        loading: true,
        error_message: None,
        zoomed_item_index: None,
    }));

    let handles = UiHandles {
        window: window.clone(),
        list_box: list_box.clone(),
        vadj,
        search_entry: search_entry.clone(),
        theme_btn: theme_btn.clone(),
        theme_icon,
        tab_all: tab_all.clone(),
        tab_pinned: tab_pinned.clone(),
        tab_images: tab_images.clone(),
        zoom_overlay: zoom_overlay.clone(),
        zoom_image,
        state,
        popover_open_count: Rc::new(Cell::new(0)),
        load_generation: Rc::new(Cell::new(0)),
    };

    handles.apply_theme();
    handles.update_filter_tabs();

    // Nút đổi Sáng / Tối
    {
        let ui = handles.clone();
        theme_btn.connect_clicked(move |_| {
            {
                let mut st = ui.state.borrow_mut();
                st.config.theme = st.config.theme.toggle();
                let _ = st.config.save();
            }
            ui.apply_theme();
        });
    }

    // Các tab lọc
    connect_filter_tab(&tab_all, FilterTab::All, handles.clone());
    connect_filter_tab(&tab_pinned, FilterTab::Pinned, handles.clone());
    connect_filter_tab(&tab_images, FilterTab::Images, handles.clone());

    // Tìm kiếm trực tiếp
    {
        let ui = handles.clone();
        search_entry.connect_search_changed(move |entry| {
            ui.state.borrow_mut().query = entry.text().to_string();
            ui.rebuild_list(0);
        });
    }

    // Click thẻ để dán ngay
    {
        let ui = handles.clone();
        list_box.connect_row_activated(move |_, row| {
            let idx = row.index();
            if idx < 0 {
                return;
            }
            let item_idx = {
                let st = ui.state.borrow();
                st.visible_indices.get(idx as usize).copied()
            };
            if let Some(item_idx) = item_idx {
                ui.paste_item(item_idx, false);
            }
        });
    }

    // Nút đóng / dán trên Zoom Overlay
    {
        let ui = handles.clone();
        zoom_close_btn.connect_clicked(move |_| ui.close_zoom());
    }
    {
        let ui = handles.clone();
        zoom_paste_btn.connect_clicked(move |_| {
            let zoomed = ui.state.borrow().zoomed_item_index;
            if let Some(item_idx) = zoomed {
                ui.paste_item(item_idx, false);
            }
        });
    }

    // Phím tắt toàn cửa sổ
    setup_keyboard_handler(&window, handles.clone());

    // Tự đóng khi click ra ngoài (có cơ chế bảo vệ khi mở menu popover ⋯)
    setup_unfocus_autoclose(&window, handles.clone());

    // Đặt tọa độ TRƯỚC khi show_all() -> cửa sổ hiện thẳng tại chỗ chuột từ khung hình đầu tiên!
    position::position_before_show(&window, pos_mode, win_w, win_h);
    window.show_all();
    zoom_overlay.hide();
    window.present();
    search_entry.grab_focus();

    // Đọc CopyQ bất đồng bộ 2 đợt ở luồng nền
    handles.reload_from_copyq();
}

impl UiHandles {
    fn apply_theme(&self) {
        let theme = self.state.borrow().config.theme;
        if let Some(settings) = Settings::default() {
            settings.set_gtk_application_prefer_dark_theme(theme == Theme::Dark);
        }
        let ctx = self.window.style_context();
        match theme {
            Theme::Dark => {
                ctx.remove_class("theme-light");
                ctx.add_class("theme-dark");
                self.theme_icon
                    .set_from_icon_name(Some("weather-clear-symbolic"), IconSize::Button);
                self.theme_icon.set_pixel_size(16);
                self.theme_btn
                    .set_tooltip_text(Some("Chuyển sang giao diện Sáng"));
            }
            Theme::Light => {
                ctx.remove_class("theme-dark");
                ctx.add_class("theme-light");
                self.theme_icon
                    .set_from_icon_name(Some("weather-clear-night-symbolic"), IconSize::Button);
                self.theme_icon.set_pixel_size(16);
                self.theme_btn
                    .set_tooltip_text(Some("Chuyển sang giao diện Tối"));
            }
        }
    }

    fn update_filter_tabs(&self) {
        let active = self.state.borrow().filter;
        set_tab_active(&self.tab_all, active == FilterTab::All);
        set_tab_active(&self.tab_pinned, active == FilterTab::Pinned);
        set_tab_active(&self.tab_images, active == FilterTab::Images);
    }

    fn reload_from_copyq(&self) {
        const FIRST_BATCH: usize = 10;

        let generation = self.load_generation.get().wrapping_add(1);
        self.load_generation.set(generation);

        let (max_items, thumb_w, image_fit) = {
            let mut st = self.state.borrow_mut();
            st.loading = true;
            let tw = (st.config.width - 76).max(180);
            (st.config.max_items, tw, st.config.image_fit)
        };

        let (tx, rx) = mpsc::channel::<(u8, Result<Vec<ClipboardItem>, String>)>();
        std::thread::spawn(move || {
            let first_limit = max_items.min(FIRST_BATCH);
            match copyq::fetch_items_range(0, first_limit) {
                Ok((total, items1)) => {
                    let has_more = total > first_limit && max_items > first_limit;
                    let _ = tx.send((1, Ok(items1)));
                    if has_more {
                        let rest_limit = max_items - first_limit;
                        if let Ok((_, items2)) = copyq::fetch_items_range(first_limit, rest_limit) {
                            let _ = tx.send((2, Ok(items2)));
                        }
                    }
                }
                Err(err) => {
                    let _ = tx.send((1, Err(err)));
                }
            }
        });

        let ui = self.clone();
        glib::timeout_add_local(Duration::from_millis(8), move || {
            if ui.load_generation.get() != generation || !ui.window.is_visible() {
                return glib::ControlFlow::Break;
            }
            if ui.popover_open_count.get() > 0 {
                return glib::ControlFlow::Continue;
            }
            loop {
                match rx.try_recv() {
                    Ok((stage, result)) => match result {
                        Ok(raw_items) => {
                            let ui_items: Vec<UiItem> = raw_items
                                .into_iter()
                                .map(|it| convert_item(it, thumb_w, image_fit))
                                .collect();
                            let keep_idx = ui
                                .list_box
                                .selected_row()
                                .and_then(|r| usize::try_from(r.index()).ok())
                                .unwrap_or(0);
                            {
                                let mut st = ui.state.borrow_mut();
                                st.loading = false;
                                st.error_message = None;
                                if stage == 1 {
                                    st.items = ui_items;
                                } else {
                                    st.items.extend(ui_items);
                                }
                            }
                            if stage == 1 {
                                ui.rebuild_list(0);
                            } else {
                                ui.rebuild_list(keep_idx);
                            }
                        }
                        Err(err) => {
                            {
                                let mut st = ui.state.borrow_mut();
                                st.loading = false;
                                st.items.clear();
                                st.error_message = Some(err);
                            }
                            ui.rebuild_list(0);
                            return glib::ControlFlow::Break;
                        }
                    },
                    Err(TryRecvError::Empty) => return glib::ControlFlow::Continue,
                    Err(TryRecvError::Disconnected) => return glib::ControlFlow::Break,
                }
            }
        });
    }

    fn rebuild_list(&self, preferred_visible_index: usize) {
        self.popover_open_count.set(0);

        let (visible, empty_text) = {
            let mut st = self.state.borrow_mut();
            let query_lower = st.query.trim().to_lowercase();
            let filter = st.filter;

            let mut indices: Vec<usize> = st
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    if filter == FilterTab::Pinned && !item.pinned {
                        return false;
                    }
                    if filter == FilterTab::Images && !matches!(item.kind, UiItemKind::Image { .. })
                    {
                        return false;
                    }
                    if !query_lower.is_empty() {
                        match &item.kind {
                            UiItemKind::Text { search, .. } => search.contains(&query_lower),
                            UiItemKind::Image { .. } => {
                                query_lower == "ảnh" || query_lower == "anh" || query_lower == "image"
                            }
                        }
                    } else {
                        true
                    }
                })
                .map(|(idx, _)| idx)
                .collect();

            // Đưa mục đã ghim lên đầu, giữ nguyên thứ tự mới nhất trong từng nhóm
            indices.sort_by_key(|&idx| !st.items[idx].pinned);
            st.visible_indices = indices.clone();

            let empty_msg = if let Some(err) = &st.error_message {
                err.clone()
            } else if st.loading {
                String::new()
            } else if !query_lower.is_empty() {
                "Không tìm thấy mục nào khớp".to_string()
            } else if filter == FilterTab::Pinned {
                "Chưa có mục nào được ghim".to_string()
            } else if filter == FilterTab::Images {
                "Chưa có ảnh nào trong lịch sử".to_string()
            } else {
                "Lịch sử CopyQ đang trống".to_string()
            };

            (indices, empty_msg)
        };

        for child in self.list_box.children() {
            self.list_box.remove(&child);
        }

        if visible.is_empty() {
            let row = ListBoxRow::new();
            row.set_selectable(false);
            row.set_activatable(false);

            let label = Label::new(Some(&empty_text));
            label.style_context().add_class("empty-label");
            label.set_line_wrap(true);
            label.set_justify(Justification::Center);
            label.set_halign(Align::Center);
            row.add(&label);
            row.show_all();
            self.list_box.add(&row);
            return;
        }

        for &item_idx in &visible {
            let row_widget = self.build_card_row(item_idx);
            row_widget.show_all();
            self.list_box.add(&row_widget);
        }

        let target = preferred_visible_index.min(visible.len().saturating_sub(1));
        if let Some(row) = self.list_box.row_at_index(target as i32) {
            self.list_box.select_row(Some(&row));
            if target == 0 {
                self.vadj.set_value(0.0);
            } else {
                self.scroll_row_into_view(&row);
            }
        }
    }

    fn build_card_row(&self, item_idx: usize) -> ListBoxRow {
        let st = self.state.borrow();
        let item = &st.items[item_idx];
        let copyq_row = item.row;
        let is_pinned = item.pinned;
        let is_image = matches!(item.kind, UiItemKind::Image { .. });

        let list_row = ListBoxRow::new();
        list_row.style_context().add_class("clip-row");
        list_row.set_selectable(true);
        list_row.set_activatable(true);

        let card_box = gtk::Box::new(Orientation::Horizontal, 6);
        if is_image {
            card_box.style_context().add_class("clip-card-image");
        } else {
            card_box.style_context().add_class("clip-card");
            card_box.set_size_request(-1, 60);
        }

        match &item.kind {
            UiItemKind::Text { display, .. } => {
                let label = Label::new(Some(display));
                label.style_context().add_class("clip-text");
                label.set_use_markup(false);
                label.set_line_wrap(true);
                label.set_line_wrap_mode(pango::WrapMode::WordChar);
                label.set_lines(3);
                label.set_ellipsize(pango::EllipsizeMode::End);
                label.set_xalign(0.0);
                label.set_yalign(0.5);
                label.set_hexpand(true);
                card_box.pack_start(&label, true, true, 0);
            }
            UiItemKind::Image { thumb, .. } => {
                let frame = gtk::Box::new(Orientation::Horizontal, 0);
                frame.style_context().add_class("img-frame");
                frame.set_size_request(-1, THUMB_HEIGHT + 8);

                if let Some(pb) = thumb {
                    let img = Image::from_pixbuf(Some(pb));
                    img.set_halign(Align::Center);
                    img.set_valign(Align::Center);
                    frame.pack_start(&img, true, true, 0);
                } else {
                    let fallback = Label::new(Some("[Không hiển thị được ảnh]"));
                    fallback.style_context().add_class("empty-label");
                    fallback.set_halign(Align::Center);
                    fallback.set_valign(Align::Center);
                    frame.pack_start(&fallback, true, true, 0);
                }

                card_box.pack_start(&frame, true, true, 0);
            }
        }

        // Cột nút xếp dọc trên-dưới (trên: ⋯ mở menu Delete, dưới: Ghim)
        let actions_col = gtk::Box::new(Orientation::Vertical, 4);
        actions_col.set_valign(Align::Fill);

        let menu_icon = Image::from_icon_name(Some("view-more-horizontal-symbolic"), IconSize::Menu);
        menu_icon.set_pixel_size(15);
        let menu_btn = MenuButton::new();
        menu_btn.style_context().add_class("card-menu-btn");
        menu_btn.set_image(Some(&menu_icon));
        menu_btn.set_always_show_image(true);
        menu_btn.set_relief(ReliefStyle::None);
        menu_btn.set_can_focus(false);
        menu_btn.set_use_popover(true);
        menu_btn.set_tooltip_text(Some("Tùy chọn"));

        let popover = Popover::new(Some(&menu_btn));
        popover.style_context().add_class("card-popover");
        popover.set_position(PositionType::Left);

        {
            let count = self.popover_open_count.clone();
            popover.connect_show(move |_| {
                count.set(count.get().saturating_add(1));
            });
        }
        {
            let count = self.popover_open_count.clone();
            popover.connect_closed(move |_| {
                count.set(count.get().saturating_sub(1));
            });
        }

        let pop_box = gtk::Box::new(Orientation::Vertical, 2);

        if is_image {
            let zoom_btn = make_popover_action("zoom-in-symbolic", "Phóng to", false);
            let ui = self.clone();
            let pop = popover.clone();
            zoom_btn.connect_clicked(move |_| {
                pop.popdown();
                ui.open_zoom(item_idx);
            });
            pop_box.pack_start(&zoom_btn, false, false, 0);
        } else {
            let plain_btn = make_popover_action("edit-paste-symbolic", "Dán chữ thuần", false);
            let ui = self.clone();
            let pop = popover.clone();
            plain_btn.connect_clicked(move |_| {
                pop.popdown();
                let ui_idle = ui.clone();
                glib::idle_add_local(move || {
                    ui_idle.paste_item(item_idx, true);
                    glib::ControlFlow::Break
                });
            });
            pop_box.pack_start(&plain_btn, false, false, 0);
        }

        let delete_btn = make_popover_action("user-trash-symbolic", "Delete", true);
        {
            let ui = self.clone();
            let pop = popover.clone();
            delete_btn.connect_clicked(move |_| {
                pop.popdown();
                let ui_idle = ui.clone();
                glib::idle_add_local(move || {
                    match copyq::delete_item(copyq_row) {
                        Ok(()) => ui_idle.reload_from_copyq(),
                        Err(err) => eprintln!("copyq-ui: {err}"),
                    }
                    glib::ControlFlow::Break
                });
            });
        }
        pop_box.pack_start(&delete_btn, false, false, 0);
        pop_box.show_all();
        popover.add(&pop_box);
        menu_btn.set_popover(Some(&popover));

        // Nút Ghim (nằm dưới nút ⋯)
        let pin_icon = Image::from_icon_name(Some("view-pin-symbolic"), IconSize::Menu);
        pin_icon.set_pixel_size(15);
        let pin_btn = Button::new();
        pin_btn.style_context().add_class("card-icon-btn");
        if is_pinned {
            pin_btn.style_context().add_class("is-pinned");
        }
        pin_btn.set_image(Some(&pin_icon));
        pin_btn.set_always_show_image(true);
        pin_btn.set_relief(ReliefStyle::None);
        pin_btn.set_can_focus(false);
        pin_btn.set_tooltip_text(Some(if is_pinned { "Bỏ ghim" } else { "Ghim lên đầu" }));
        {
            let ui = self.clone();
            pin_btn.connect_clicked(move |_| {
                let ui_idle = ui.clone();
                glib::idle_add_local(move || {
                    match copyq::toggle_pin(copyq_row) {
                        Ok(()) => ui_idle.reload_from_copyq(),
                        Err(err) => eprintln!("copyq-ui: {err}"),
                    }
                    glib::ControlFlow::Break
                });
            });
        }

        actions_col.pack_start(&menu_btn, false, false, 0);
        actions_col.pack_end(&pin_btn, false, false, 0);

        card_box.pack_end(&actions_col, false, false, 0);
        list_row.add(&card_box);
        list_row
    }

    fn selected_item_index(&self) -> Option<usize> {
        let row = self.list_box.selected_row()?;
        let vis_idx = usize::try_from(row.index()).ok()?;
        self.state.borrow().visible_indices.get(vis_idx).copied()
    }

    fn move_selection(&self, delta: isize) {
        let count = self.state.borrow().visible_indices.len();
        if count == 0 {
            return;
        }
        let current = self
            .list_box
            .selected_row()
            .and_then(|r| usize::try_from(r.index()).ok())
            .unwrap_or(0);
        let next = ((current as isize + delta).rem_euclid(count as isize)) as usize;
        if let Some(row) = self.list_box.row_at_index(next as i32) {
            self.list_box.select_row(Some(&row));
            self.scroll_row_into_view(&row);
        }
    }

    fn scroll_row_into_view(&self, row: &ListBoxRow) {
        let alloc = row.allocation();
        if alloc.height() <= 0 {
            return;
        }
        let adj = &self.vadj;
        let top = f64::from(alloc.y());
        let bottom = top + f64::from(alloc.height());
        let view_top = adj.value();
        let view_bottom = view_top + adj.page_size();

        if top < view_top {
            adj.set_value(top);
        } else if bottom > view_bottom {
            adj.set_value((bottom - adj.page_size()).max(0.0));
        }
    }

    fn open_zoom(&self, item_idx: usize) {
        let (pixbuf, max_w, max_h) = {
            let st = self.state.borrow();
            let pb = match st.items.get(item_idx) {
                Some(UiItem {
                    kind: UiItemKind::Image { full: Some(pb), .. },
                    ..
                }) => Some(pb.clone()),
                _ => None,
            };
            (
                pb,
                (st.config.width - 40).max(200),
                (st.config.height - 120).max(200),
            )
        };

        if let Some(pb) = pixbuf {
            if let Some(scaled) = scale_pixbuf(&pb, max_w, max_h, false) {
                self.zoom_image.set_from_pixbuf(Some(&scaled));
            } else {
                self.zoom_image.set_from_pixbuf(Some(&pb));
            }
            self.state.borrow_mut().zoomed_item_index = Some(item_idx);
            self.zoom_overlay.show();
        }
    }

    fn close_zoom(&self) {
        self.state.borrow_mut().zoomed_item_index = None;
        self.zoom_overlay.hide();
        self.zoom_image.clear();
    }

    fn paste_item(&self, item_idx: usize, force_plain_text: bool) {
        let (copyq_row, auto_paste, plain_text) = {
            let st = self.state.borrow();
            let Some(item) = st.items.get(item_idx) else {
                return;
            };
            let is_text = matches!(item.kind, UiItemKind::Text { .. });
            let use_plain = is_text && (force_plain_text || st.config.plain_text_paste);
            (item.row, st.config.auto_paste, use_plain)
        };

        let _ = copyq::select_and_paste(copyq_row, auto_paste, plain_text);
        self.window.hide();
        self.window.close();
    }
}

fn convert_item(item: ClipboardItem, thumb_w: i32, image_fit: ImageFit) -> UiItem {
    let kind = match item.content {
        ItemContent::Text { display, search } => UiItemKind::Text { display, search },
        ItemContent::Image { bytes } => {
            let full = decode_pixbuf(&bytes);
            let cover = image_fit == ImageFit::Cover;
            let thumb = full
                .as_ref()
                .and_then(|pb| scale_pixbuf(pb, thumb_w, THUMB_HEIGHT, cover));
            UiItemKind::Image { thumb, full }
        }
    };
    UiItem {
        row: item.row,
        pinned: item.pinned,
        kind,
    }
}

fn decode_pixbuf(bytes: &[u8]) -> Option<Pixbuf> {
    let loader = PixbufLoader::new();
    loader.write(bytes).ok()?;
    loader.close().ok()?;
    loader.pixbuf()
}

fn scale_pixbuf(pb: &Pixbuf, max_w: i32, max_h: i32, cover: bool) -> Option<Pixbuf> {
    let tw = pb.width().max(1);
    let th = pb.height().max(1);
    let max_w = max_w.max(1);
    let max_h = max_h.max(1);

    if cover {
        let scale = (f64::from(max_w) / f64::from(tw)).max(f64::from(max_h) / f64::from(th));
        let w = ((f64::from(tw) * scale).round() as i32).max(max_w);
        let h = ((f64::from(th) * scale).round() as i32).max(max_h);
        let scaled = pb.scale_simple(w, h, InterpType::Bilinear)?;
        let x = ((w - max_w) / 2).max(0);
        let y = ((h - max_h) / 2).max(0);
        let cw = max_w.min(w - x).max(1);
        let ch = max_h.min(h - y).max(1);
        Some(scaled.new_subpixbuf(x, y, cw, ch))
    } else {
        let scale = (f64::from(max_w) / f64::from(tw)).min(f64::from(max_h) / f64::from(th));
        let w = ((f64::from(tw) * scale).round() as i32).clamp(1, max_w);
        let h = ((f64::from(th) * scale).round() as i32).clamp(1, max_h);
        pb.scale_simple(w, h, InterpType::Bilinear)
    }
}

fn create_copyq_logo() -> Image {
    let has_icon = IconTheme::default()
        .map(|theme| theme.has_icon("copyq"))
        .unwrap_or(false);
    if has_icon {
        let img = Image::from_icon_name(Some("copyq"), IconSize::LargeToolbar);
        img.set_pixel_size(24);
        return img;
    }

    const COPYQ_SVG: &str = "/usr/share/icons/hicolor/scalable/apps/copyq.svg";
    if Path::new(COPYQ_SVG).is_file() {
        if let Ok(pb) = Pixbuf::from_file_at_scale(COPYQ_SVG, 24, 24, true) {
            return Image::from_pixbuf(Some(&pb));
        }
    }

    let img = Image::from_icon_name(Some("edit-paste-symbolic"), IconSize::LargeToolbar);
    img.set_pixel_size(24);
    img
}

fn make_draggable(child: &impl IsA<gtk::Widget>, window: &ApplicationWindow) -> EventBox {
    let eb = EventBox::new();
    eb.set_visible_window(false);
    eb.add(child);
    let win = window.clone();
    eb.connect_button_press_event(move |_, ev| {
        if ev.button() == 1 && ev.event_type() == gdk::EventType::ButtonPress {
            let (rx, ry) = ev.root();
            win.begin_move_drag(1, rx as i32, ry as i32, ev.time());
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    eb
}

fn make_filter_button(label: &str) -> Button {
    let btn = Button::with_label(label);
    btn.style_context().add_class("filter-tab");
    btn.set_relief(ReliefStyle::None);
    btn.set_can_focus(false);
    btn
}

fn set_tab_active(btn: &Button, active: bool) {
    let ctx = btn.style_context();
    if active {
        ctx.add_class("is-active");
    } else {
        ctx.remove_class("is-active");
    }
}

fn connect_filter_tab(btn: &Button, tab: FilterTab, ui: UiHandles) {
    btn.connect_clicked(move |_| {
        {
            let mut st = ui.state.borrow_mut();
            if st.filter == tab {
                return;
            }
            st.filter = tab;
        }
        ui.update_filter_tabs();
        ui.rebuild_list(0);
    });
}

fn make_popover_action(icon_name: &str, label_text: &str, danger: bool) -> Button {
    let content = gtk::Box::new(Orientation::Horizontal, 8);
    let icon = Image::from_icon_name(Some(icon_name), IconSize::Menu);
    icon.set_pixel_size(14);
    let label = Label::new(Some(label_text));
    label.set_xalign(0.0);
    content.pack_start(&icon, false, false, 0);
    content.pack_start(&label, true, true, 0);

    let btn = Button::new();
    btn.style_context().add_class("popover-action-btn");
    if danger {
        btn.style_context().add_class("danger-btn");
    }
    btn.set_relief(ReliefStyle::None);
    btn.add(&content);
    btn
}

fn build_zoom_overlay() -> (gtk::Box, Image, Button, Button) {
    let overlay_box = gtk::Box::new(Orientation::Vertical, 10);
    overlay_box.style_context().add_class("zoom-overlay");
    overlay_box.set_vexpand(true);
    overlay_box.set_hexpand(true);

    let top_bar = gtk::Box::new(Orientation::Horizontal, 8);
    let title = Label::new(Some("Xem trước ảnh"));
    title.style_context().add_class("zoom-title");
    title.set_xalign(0.0);

    let close_btn = Button::with_label("Đóng (Esc)");
    close_btn.style_context().add_class("zoom-close-btn");
    close_btn.set_relief(ReliefStyle::None);

    top_bar.pack_start(&title, true, true, 0);
    top_bar.pack_end(&close_btn, false, false, 0);

    let zoom_frame = gtk::Box::new(Orientation::Vertical, 0);
    zoom_frame.style_context().add_class("zoom-frame");
    zoom_frame.set_vexpand(true);
    zoom_frame.set_hexpand(true);

    let zoom_image = Image::new();
    zoom_image.set_halign(Align::Center);
    zoom_image.set_valign(Align::Center);
    zoom_frame.pack_start(&zoom_image, true, true, 0);

    let paste_btn = Button::with_label("Dán ảnh này (Enter)");
    paste_btn.style_context().add_class("zoom-paste-btn");
    paste_btn.set_relief(ReliefStyle::None);

    overlay_box.pack_start(&top_bar, false, false, 0);
    overlay_box.pack_start(&zoom_frame, true, true, 0);
    overlay_box.pack_end(&paste_btn, false, false, 0);

    overlay_box.show_all();
    overlay_box.set_no_show_all(true);
    overlay_box.hide();

    (overlay_box, zoom_image, close_btn, paste_btn)
}

fn setup_keyboard_handler(window: &ApplicationWindow, ui: UiHandles) {
    window.connect_key_press_event(move |_, ev| {
        let key = ev.keyval();
        let modifiers = ev.state();

        let zoomed = ui.state.borrow().zoomed_item_index;
        if let Some(item_idx) = zoomed {
            if key == gdk::keys::constants::Escape || key == gdk::keys::constants::space {
                ui.close_zoom();
                return glib::Propagation::Stop;
            }
            if key == gdk::keys::constants::Return || key == gdk::keys::constants::KP_Enter {
                ui.paste_item(item_idx, false);
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Stop;
        }

        if modifiers.contains(gdk::ModifierType::CONTROL_MASK)
            && (key == gdk::keys::constants::f || key == gdk::keys::constants::F)
        {
            ui.search_entry.grab_focus();
            return glib::Propagation::Stop;
        }

        if key == gdk::keys::constants::Escape {
            if ui.popover_open_count.get() > 0 {
                return glib::Propagation::Proceed;
            }
            if !ui.search_entry.text().is_empty() {
                ui.search_entry.set_text("");
                ui.search_entry.grab_focus();
            } else {
                ui.window.close();
            }
            return glib::Propagation::Stop;
        }

        if key == gdk::keys::constants::Down {
            ui.move_selection(1);
            return glib::Propagation::Stop;
        }

        if key == gdk::keys::constants::Up {
            ui.move_selection(-1);
            return glib::Propagation::Stop;
        }

        if key == gdk::keys::constants::Return || key == gdk::keys::constants::KP_Enter {
            if let Some(item_idx) = ui.selected_item_index() {
                let force_plain = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
                ui.paste_item(item_idx, force_plain);
            }
            return glib::Propagation::Stop;
        }

        if key == gdk::keys::constants::space {
            let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
            let search_empty = ui.search_entry.text().is_empty();
            if ctrl || search_empty {
                if let Some(item_idx) = ui.selected_item_index() {
                    let is_img = matches!(
                        ui.state.borrow().items.get(item_idx),
                        Some(UiItem {
                            kind: UiItemKind::Image { .. },
                            ..
                        })
                    );
                    if is_img {
                        ui.open_zoom(item_idx);
                        return glib::Propagation::Stop;
                    }
                }
            }
        }

        glib::Propagation::Proceed
    });
}

fn setup_unfocus_autoclose(window: &ApplicationWindow, ui: UiHandles) {
    let close_on_unfocus = ui.state.borrow().config.close_on_unfocus;
    if !close_on_unfocus {
        return;
    }

    let has_been_active = Rc::new(Cell::new(false));
    window.connect_is_active_notify(move |win| {
        if win.is_active() {
            has_been_active.set(true);
            return;
        }
        if !has_been_active.get() {
            return;
        }
        let win_weak = win.downgrade();
        let pop_count = ui.popover_open_count.clone();
        glib::timeout_add_local(Duration::from_millis(120), move || {
            if let Some(w) = win_weak.upgrade() {
                if !w.is_active() && pop_count.get() == 0 {
                    w.close();
                }
            }
            glib::ControlFlow::Break
        });
    });
}

fn load_stylesheet() {
    let Some(screen) = gdk::Screen::default() else {
        return;
    };

    let base_provider = CssProvider::new();
    let _ = base_provider.load_from_data(DEFAULT_CSS.as_bytes());
    StyleContext::add_provider_for_screen(
        &screen,
        &base_provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    if let Some(custom_css) = AppConfig::load_or_create_custom_css() {
        let user_provider = CssProvider::new();
        let _ = user_provider.load_from_data(custom_css.as_bytes());
        StyleContext::add_provider_for_screen(
            &screen,
            &user_provider,
            gtk::STYLE_PROVIDER_PRIORITY_USER,
        );
    }
}
