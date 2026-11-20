# CopyQ-UI

Giao diện popup kiểu **`Win + V` (Windows 11)** dành cho **CopyQ** trên **Linux Mint Cinnamon (X11)**, viết bằng **Rust + GTK3**.

## Điểm nổi bật
- **Gọn, mượt, mở ngay chỗ chuột:** Tự lật tránh mép màn hình & taskbar, ẩn khỏi thanh taskbar, kéo di chuyển tự do.
- **Siêu nhẹ — không cần chạy ngầm:** Dùng chung GTK3 có sẵn của Cinnamon (~15 MB RAM thực khi mở, **0 MB khi đóng**). Việc lưu lịch sử vẫn do CopyQ gốc đảm nhiệm.
- **Đầy đủ thao tác nhanh:** Tìm kiếm tức thì, lọc *Tất cả / Đã ghim / Ảnh*, đổi chế độ **Sáng / Tối**, ghim mục lên đầu, phóng to xem ảnh (`Space`), dán chữ thuần không dính định dạng web (`Shift + Enter`).
- **An toàn & 100% Offline:** `#![forbid(unsafe_code)]`, chỉ dùng đúng 1 thư viện `gtk`, không kết nối mạng, không gọi shell, không ghi dữ liệu clipboard ra ổ cứng.

---

## Yêu cầu
- **CopyQ** đã cài đặt và đang chạy nền (bật *Autostart* trong CopyQ).
- Linux dùng X11 có sẵn GTK3 *(đã kiểm thử ổn định trên Linux Mint 22.x Cinnamon)*.

---

## Cách cài đặt & gắn phím `Win + V`

### 1. Lấy file chạy `copyq-ui`
- **Cách nhanh:** Tải bản build sẵn ở mục **Releases** hoặc tab **Actions** → artifact `copyq-ui-linux-x86_64`.
- **Hoặc tự build từ mã nguồn** *(Ubuntu 24.04 / Linux Mint 22.x)*:
  ```bash
  sudo apt install -y libgtk-3-dev pkg-config
  cargo build --release
  # File chạy nằm tại: target/release/copyq-ui
  ```

### 2. Chép vào `~/.local/bin` và cấp quyền
```bash
mkdir -p ~/.local/bin
cp copyq-ui ~/.local/bin/copyq-ui
chmod +x ~/.local/bin/copyq-ui
```

### 3. Gắn phím tắt `Win + V` (Linux Mint Cinnamon)
Mở **System Settings → Keyboard → Shortcuts → Custom Shortcuts → Add custom shortcut**:
- **Name:** `CopyQ UI`
- **Command:** `/home/<tên-user>/.local/bin/copyq-ui` *(điền đầy đủ đường dẫn, không viết tắt `~`)*
- **Keyboard bindings:** gán tổ hợp **`Win + V`** (`Super+V`).

---

## Phím tắt trong popup
| Phím | Chức năng |
| :--- | :--- |
| `↑` / `↓` | Chọn mục |
| `Enter` hoặc Click | Dán mục đang chọn & tự đóng |
| `Shift + Enter` | Dán dạng **chữ thuần** (bỏ định dạng web) |
| `Space` | Phóng to / đóng xem trước ảnh đang chọn |
| `Ctrl + F` | Tập trung vào ô tìm kiếm |
| `Esc` / `Win + V` | Đóng xem ảnh / xóa từ khóa / đóng popup |

---

## Tùy chỉnh (tùy chọn)
Lần chạy đầu tiên, app tự tạo 2 file trong `~/.config/copyq-ui/`:
- `config.conf`: đổi theme (`dark`/`light`), kích thước cửa sổ, số mục tối đa, vị trí mở (`cursor`, `bottom-right`, `center`), cách hiển thị ảnh (`contain`/`cover`)...
- `custom.css`: tùy chỉnh màu sắc, bo góc, cỡ chữ — lưu xong bấm `Win + V` là áp dụng ngay, không cần build lại.

## Gỡ cài đặt
Không ảnh hưởng đến dữ liệu CopyQ gốc:
```bash
rm -f ~/.local/bin/copyq-ui && rm -rf ~/.config/copyq-ui
```
Sau đó xóa (hoặc đổi lại) phím tắt `Super + V` trong **Keyboard → Shortcuts**.
