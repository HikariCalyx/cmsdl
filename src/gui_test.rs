/// GUI test window – per-pixel alpha via UpdateLayeredWindow.
///
/// Window body:   src/gui_res/139.png  (626 × 583, has alpha/rounded corners)
/// Close button:  150.bmp normal  151.bmp hover  152.bmp click  (24 × 24 each)
/// Progress bar:  154.bmp  (63 × 12, tiled horizontally)
///
/// WS_EX_LAYERED + UpdateLayeredWindow(ULW_ALPHA) gives us per-pixel
/// transparency so the rounded corners show through to the desktop.

pub fn run_gui_test() -> anyhow::Result<()> {
    #[cfg(windows)]
    { win32::run() }
    #[cfg(not(windows))]
    { anyhow::bail!("gui_test is only supported on Windows") }
}

#[cfg(windows)]
mod win32 {
    use std::ffi::c_void;
    use std::ptr;

    // ── Embedded resources ───────────────────────────────────────────────────
    const BG_PNG: &[u8]       = include_bytes!("gui_res/139.png");
    const BMP_NORMAL: &[u8]   = include_bytes!("gui_res/150.bmp");
    const BMP_HOVER: &[u8]    = include_bytes!("gui_res/151.bmp");
    const BMP_CLICK: &[u8]    = include_bytes!("gui_res/152.bmp");
    const BMP_PROGRESS: &[u8] = include_bytes!("gui_res/154.bmp");

    // ── Layout ───────────────────────────────────────────────────────────────
    const WIN_W: i32 = 626;
    const WIN_H: i32 = 583;
    const BTN_W: i32 = 24;
    const BTN_H: i32 = 24;
    const BTN_X: i32 = WIN_W - BTN_W - 8;
    const BTN_Y: i32 = 8;
    const PROG_TILE_W: i32 = 63;
    const PROG_H: i32 = 12;
    const PROG_RENDERED_W: i32 = 400;
    const PROG_X: i32 = (WIN_W - PROG_RENDERED_W) / 2;
    const PROG_Y: i32 = WIN_H - 60;

    // ── Win32 types ──────────────────────────────────────────────────────────
    type HWND      = *mut c_void;
    type HDC       = *mut c_void;
    type HBMP      = isize;
    type HGDIOBJ   = isize;
    type HINSTANCE = *mut c_void;
    type HMENU     = *mut c_void;
    type HICON     = *mut c_void;
    type HCURSOR   = *mut c_void;
    type HBRUSH    = *mut c_void;
    type ATOM      = u16;
    type LRESULT   = isize;
    type WPARAM    = usize;
    type LPARAM    = isize;
    type BOOL      = i32;

    // Messages
    const WM_DESTROY:     u32 = 0x0002;
    const WM_LBUTTONDOWN: u32 = 0x0201;
    const WM_LBUTTONUP:   u32 = 0x0202;
    const WM_MOUSEMOVE:   u32 = 0x0200;
    const WM_MOUSELEAVE:  u32 = 0x02A3;
    const WM_NCHITTEST:   u32 = 0x0084;
    const WM_SETCURSOR:   u32 = 0x0020;
    const HTCLIENT:  isize = 1;
    const HTCAPTION: isize = 2;
    const TME_LEAVE: u32 = 0x0002;

    // GDI / layered window
    #[allow(dead_code)]
    const SRCCOPY:       u32 = 0x00CC0020;
    const ULW_ALPHA:     u32 = 0x0002;
    const AC_SRC_OVER:   u8  = 0x00;
    const AC_SRC_ALPHA:  u8  = 0x01;
    const BI_RGB:        u32 = 0;
    const DIB_RGB_COLORS:u32 = 0;

    // Window styles
    const WS_POPUP:         u32 = 0x80000000;
    const WS_EX_APPWINDOW:  u32 = 0x00040000;
    const WS_EX_LAYERED:    u32 = 0x00080000;

    const GWLP_USERDATA: i32 = -21;
    const IDC_ARROW:   usize = 32512;
    #[allow(dead_code)]
    const IMAGE_BITMAP:  u32 = 0;
    #[allow(dead_code)]
    const LR_LOADFROMFILE: u32 = 0x0010;

    // ── FFI structs ──────────────────────────────────────────────────────────
    #[repr(C)]
    struct WNDCLASSEXW {
        cb_size:         u32,
        style:           u32,
        lpfn_wnd_proc:   extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
        cb_cls_extra:    i32,
        cb_wnd_extra:    i32,
        h_instance:      HINSTANCE,
        h_icon:          HICON,
        h_cursor:        HCURSOR,
        hbr_background:  HBRUSH,
        lpsz_menu_name:  *const u16,
        lpsz_class_name: *const u16,
        h_icon_sm:       HICON,
    }

    #[repr(C)]
    struct MSG {
        hwnd: HWND, message: u32, w_param: WPARAM, l_param: LPARAM,
        time: u32, pt_x: i32, pt_y: i32, private: u32,
    }

    #[repr(C)]
    struct TRACKMOUSEEVENT {
        cb_size: u32, dw_flags: u32, hwnd_track: HWND, dw_hover_time: u32,
    }

    /// BLENDFUNCTION for UpdateLayeredWindow
    #[repr(C)]
    struct BLENDFUNCTION {
        blend_op:             u8,
        blend_flags:          u8,
        source_constant_alpha: u8,
        alpha_format:         u8,
    }

    /// POINT (screen coordinates)
    #[repr(C)]
    struct POINT { x: i32, y: i32 }

    /// SIZE
    #[repr(C)]
    struct SIZE { cx: i32, cy: i32 }

    /// BITMAPINFOHEADER
    #[repr(C)]
    struct BITMAPINFOHEADER {
        bi_size:           u32,
        bi_width:          i32,
        bi_height:         i32,
        bi_planes:         u16,
        bi_bit_count:      u16,
        bi_compression:    u32,
        bi_size_image:     u32,
        bi_x_pels_per_meter: i32,
        bi_y_pels_per_meter: i32,
        bi_clr_used:       u32,
        bi_clr_important:  u32,
    }

    /// BITMAPINFO (no palette)
    #[repr(C)]
    struct BITMAPINFO {
        bmi_header: BITMAPINFOHEADER,
        bmi_colors: [u32; 1], // unused for 32-bit
    }

    #[repr(C)]
    struct GdiplusStartupInput {
        version: u32, debug_event_callback: *const c_void,
        suppress_background_thread: BOOL, suppress_external_codecs: BOOL,
    }

    // ── FFI imports ──────────────────────────────────────────────────────────
    #[allow(dead_code)]
    #[link(name = "user32")]
    extern "system" {
        fn RegisterClassExW(wc: *const WNDCLASSEXW) -> ATOM;
        fn CreateWindowExW(ex: u32, cls: *const u16, name: *const u16,
            style: u32, x: i32, y: i32, w: i32, h: i32,
            parent: HWND, menu: HMENU, inst: HINSTANCE, param: *mut c_void) -> HWND;
        fn GetMessageW(msg: *mut MSG, hwnd: HWND, min: u32, max: u32) -> BOOL;
        fn TranslateMessage(msg: *const MSG) -> BOOL;
        fn DispatchMessageW(msg: *const MSG) -> LRESULT;
        fn PostQuitMessage(code: i32);
        fn DefWindowProcW(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT;
        fn GetWindowLongPtrW(hwnd: HWND, idx: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: HWND, idx: i32, val: isize) -> isize;
        fn TrackMouseEvent(tme: *mut TRACKMOUSEEVENT) -> BOOL;
        fn LoadCursorW(inst: HINSTANCE, name: usize) -> HCURSOR;
        fn SetCursor(cursor: HCURSOR) -> HCURSOR;
        fn GetSystemMetrics(idx: i32) -> i32;
        fn GetWindowRect(hwnd: HWND, rect: *mut [i32; 4]) -> BOOL;
        fn UpdateLayeredWindow(hwnd: HWND, hdc_dst: HDC, pt_dst: *const POINT,
            size: *const SIZE, hdc_src: HDC, pt_src: *const POINT,
            key: u32, blend: *const BLENDFUNCTION, flags: u32) -> BOOL;
    }

    #[link(name = "gdi32")]
    extern "system" {
        fn GetDC(hwnd: HWND) -> HDC;
        fn ReleaseDC(hwnd: HWND, hdc: HDC) -> i32;
        fn CreateCompatibleDC(hdc: HDC) -> HDC;
        fn SelectObject(hdc: HDC, obj: HGDIOBJ) -> HGDIOBJ;
        #[allow(dead_code)]
        fn BitBlt(dst: HDC, x: i32, y: i32, w: i32, h: i32,
            src: HDC, sx: i32, sy: i32, rop: u32) -> BOOL;
        fn DeleteDC(hdc: HDC) -> BOOL;
        fn DeleteObject(obj: HGDIOBJ) -> BOOL;
        fn CreateDIBSection(hdc: HDC, bmi: *const BITMAPINFO, usage: u32,
            bits: *mut *mut u8, section: *mut c_void, offset: u32) -> HBMP;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> HINSTANCE;
    }

    // GDI+ — dynamically present on all Windows versions since XP
    extern "system" {
        fn GdiplusStartup(token: *mut usize, input: *const GdiplusStartupInput,
            output: *mut c_void) -> i32;
        fn GdipCreateBitmapFromFile(file: *const u16, bmp: *mut *mut c_void) -> i32;
        fn GdipBitmapLockBits(bmp: *mut c_void, rect: *const [i32;4], flags: u32,
            format: i32, data: *mut BitmapData) -> i32;
        fn GdipBitmapUnlockBits(bmp: *mut c_void, data: *mut BitmapData) -> i32;
        fn GdipDisposeImage(img: *mut c_void) -> i32;
        fn GdipGetImageWidth(img: *mut c_void, w: *mut u32) -> i32;
        fn GdipGetImageHeight(img: *mut c_void, h: *mut u32) -> i32;
    }

    /// GDI+ BitmapData structure
    #[repr(C)]
    struct BitmapData {
        width:       u32,
        height:      u32,
        stride:      i32,
        pixel_format: i32,
        scan0:       *mut u8,
        reserved:    usize,
    }

    // PixelFormat32bppARGB = 0x26200A (GDI+ constant)
    const PIXEL_FORMAT_32BPP_ARGB: i32 = 0x26200A;
    const IMAGING_LOCK_READ: u32 = 1;

    // ── State ────────────────────────────────────────────────────────────────
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum BtnState { Normal, Hover, Pressed }

    struct WindowState {
        hwnd:          HWND,
        /// Screen position (top-left) of the window.
        win_x:         i32,
        win_y:         i32,
        /// ARGB pixel rows for the background (pre-multiplied not required by
        /// UpdateLayeredWindow when AC_SRC_ALPHA is set, but the PNG pixels are
        /// already straight alpha — we pre-multiply here).
        bg_pixels:     Vec<u32>,   // WIN_W * WIN_H, row-major top-down
        btn_pixels:    [Vec<u32>; 3], // Normal/Hover/Pressed  (BTN_W*BTN_H each)
        prog_pixels:   Vec<u32>,   // PROG_TILE_W * PROG_H
        btn_state:     BtnState,
        /// 0–100
        progress:      u32,
    }

    impl WindowState {
        fn hit_close(&self, x: i32, y: i32) -> bool {
            x >= BTN_X && x < BTN_X + BTN_W && y >= BTN_Y && y < BTN_Y + BTN_H
        }
    }

    // ── GDI+ helpers ─────────────────────────────────────────────────────────

    static GDIP_INITED: std::sync::Once = std::sync::Once::new();

    fn gdip_init() {
        GDIP_INITED.call_once(|| {
            let input = GdiplusStartupInput {
                version: 1,
                debug_event_callback: ptr::null(),
                suppress_background_thread: 0,
                suppress_external_codecs: 0,
            };
            let mut token: usize = 0;
            unsafe { GdiplusStartup(&mut token, &input, ptr::null_mut()) };
        });
    }

    /// Load a PNG/BMP from bytes, lock its bits in ARGB32 order, return
    /// a Vec<u32> of pre-multiplied ARGB pixels (top-down, row-major).
    fn load_pixels(data: &[u8], ext: &str) -> Vec<u32> {
        gdip_init();

        let ext = if ext.starts_with('.') { ext.to_owned() } else { format!(".{ext}") };
        let tmp = {
            let mut p = std::env::temp_dir();
            p.push(format!("cmsdl_gui_{:x}{ext}", data.as_ptr() as usize));
            p
        };
        let _ = std::fs::write(&tmp, data);
        let wide: Vec<u16> = tmp.to_string_lossy().encode_utf16().chain(Some(0)).collect();

        let mut img: *mut c_void = ptr::null_mut();
        let ok = unsafe { GdipCreateBitmapFromFile(wide.as_ptr(), &mut img) };
        let _ = std::fs::remove_file(&tmp);
        if ok != 0 || img.is_null() { return Vec::new(); }

        let mut w = 0u32;
        let mut h = 0u32;
        unsafe {
            GdipGetImageWidth(img, &mut w);
            GdipGetImageHeight(img, &mut h);
        }

        let mut bd = BitmapData {
            width: 0, height: 0, stride: 0,
            pixel_format: 0, scan0: ptr::null_mut(), reserved: 0,
        };
        let rect = [0i32, 0, w as i32, h as i32];
        let lock_ok = unsafe {
            GdipBitmapLockBits(img, &rect, IMAGING_LOCK_READ, PIXEL_FORMAT_32BPP_ARGB, &mut bd)
        };
        if lock_ok != 0 { unsafe { GdipDisposeImage(img) }; return Vec::new(); }

        let pixel_count = (w * h) as usize;
        let mut pixels = Vec::with_capacity(pixel_count);

        for row in 0..h as i32 {
            let row_ptr = unsafe { bd.scan0.offset((row * bd.stride) as isize) as *const u32 };
            for col in 0..w {
                // GDI+ ARGB: 0xAARRGGBB  — pre-multiply for UpdateLayeredWindow
                let argb = unsafe { *row_ptr.add(col as usize) };
                let a = (argb >> 24) & 0xFF;
                if a == 0xFF {
                    pixels.push(argb);
                } else {
                    // Pre-multiply RGB channels by alpha/255
                    let r = (((argb >> 16) & 0xFF) * a / 255) & 0xFF;
                    let g = (((argb >>  8) & 0xFF) * a / 255) & 0xFF;
                    let b = (( argb        & 0xFF) * a / 255) & 0xFF;
                    pixels.push((a << 24) | (r << 16) | (g << 8) | b);
                }
            }
        }

        unsafe {
            GdipBitmapUnlockBits(img, &mut bd);
            GdipDisposeImage(img);
        }
        pixels
    }

    // ── Compositing & UpdateLayeredWindow ────────────────────────────────────

    /// Build a 32-bit DIB, composite all layers into it, then call
    /// UpdateLayeredWindow so the OS uses per-pixel alpha against the desktop.
    fn update_layered(s: &WindowState) {
        // Create a 32-bit top-down DIB to paint into.
        let bmi = BITMAPINFO {
            bmi_header: BITMAPINFOHEADER {
                bi_size:             std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                bi_width:            WIN_W,
                bi_height:           -WIN_H,   // negative = top-down
                bi_planes:           1,
                bi_bit_count:        32,
                bi_compression:      BI_RGB,
                bi_size_image:       0,
                bi_x_pels_per_meter: 0,
                bi_y_pels_per_meter: 0,
                bi_clr_used:         0,
                bi_clr_important:    0,
            },
            bmi_colors: [0],
        };

        let mut dib_bits: *mut u8 = ptr::null_mut();
        let hdc_screen = unsafe { GetDC(ptr::null_mut()) };
        let hdc_mem    = unsafe { CreateCompatibleDC(hdc_screen) };
        let hbmp = unsafe {
            CreateDIBSection(hdc_screen, &bmi, DIB_RGB_COLORS,
                &mut dib_bits, ptr::null_mut(), 0)
        };
        if hbmp == 0 || dib_bits.is_null() {
            unsafe { DeleteDC(hdc_mem); ReleaseDC(ptr::null_mut(), hdc_screen); }
            return;
        }
        unsafe { SelectObject(hdc_mem, hbmp) };

        // The DIB pixel buffer is in GDI's native BGRA order.
        // Our pixel vecs are pre-multiplied ARGB (0xAARRGGBB).
        // We need to write BGRA bytes: [B, G, R, A].
        let dib = unsafe {
            std::slice::from_raw_parts_mut(dib_bits as *mut u32,
                (WIN_W * WIN_H) as usize)
        };

        // Helper: write one pre-multiplied ARGB pixel as a DIB BGRA dword.
        // GDI DIBs store bytes in memory as B, G, R, A
        // i.e. the u32 in little-endian is: 0xAARRGGBB → swap R and B.
        let argb_to_bgra = |argb: u32| -> u32 {
            let a = (argb >> 24) & 0xFF;
            let r = (argb >> 16) & 0xFF;
            let g = (argb >>  8) & 0xFF;
            let b =  argb        & 0xFF;
            (a << 24) | (r << 16) | (g << 8) | b
            // wait — DIB BGRA means bytes [B,G,R,A], which as a little-endian u32
            // is B | G<<8 | R<<16 | A<<24
        };
        let argb_to_dib = |argb: u32| -> u32 {
            let a = (argb >> 24) & 0xFF;
            let r = (argb >> 16) & 0xFF;
            let g = (argb >>  8) & 0xFF;
            let b =  argb        & 0xFF;
            b | (g << 8) | (r << 16) | (a << 24)
        };
        let _ = argb_to_bgra; // silence unused warning

        // 1. Copy background.
        let bg = &s.bg_pixels;
        let bg_len = bg.len().min(dib.len());
        for i in 0..bg_len {
            dib[i] = argb_to_dib(bg[i]);
        }

        // 2. Composite close button over background.
        let btn_px = match s.btn_state {
            BtnState::Normal  => &s.btn_pixels[0],
            BtnState::Hover   => &s.btn_pixels[1],
            BtnState::Pressed => &s.btn_pixels[2],
        };
        for by in 0..BTN_H {
            for bx in 0..BTN_W {
                let dst_x = BTN_X + bx;
                let dst_y = BTN_Y + by;
                if dst_x < 0 || dst_x >= WIN_W || dst_y < 0 || dst_y >= WIN_H { continue; }
                let src_idx = (by * BTN_W + bx) as usize;
                if src_idx >= btn_px.len() { continue; }
                let dst_idx = (dst_y * WIN_W + dst_x) as usize;
                // Alpha-blend button pixel over current DIB pixel.
                dib[dst_idx] = alpha_blend_dib(dib[dst_idx], argb_to_dib(btn_px[src_idx]));
            }
        }

        // 3. Composite progress bar (tile).
        let filled = (PROG_RENDERED_W * s.progress.min(100) as i32) / 100;
        for py in 0..PROG_H {
            for px in 0..filled {
                let dst_x = PROG_X + px;
                let dst_y = PROG_Y + py;
                if dst_x < 0 || dst_x >= WIN_W || dst_y < 0 || dst_y >= WIN_H { continue; }
                let src_x = px % PROG_TILE_W;
                let src_idx = (py * PROG_TILE_W + src_x) as usize;
                if src_idx >= s.prog_pixels.len() { continue; }
                let dst_idx = (dst_y * WIN_W + dst_x) as usize;
                dib[dst_idx] = alpha_blend_dib(dib[dst_idx], argb_to_dib(s.prog_pixels[src_idx]));
            }
        }

        // Call UpdateLayeredWindow.
        let pt_dst = POINT { x: s.win_x, y: s.win_y };
        let sz     = SIZE  { cx: WIN_W, cy: WIN_H };
        let pt_src = POINT { x: 0, y: 0 };
        let blend  = BLENDFUNCTION {
            blend_op:              AC_SRC_OVER,
            blend_flags:           0,
            source_constant_alpha: 255,
            alpha_format:          AC_SRC_ALPHA,
        };
        unsafe {
            UpdateLayeredWindow(s.hwnd, hdc_screen, &pt_dst, &sz,
                hdc_mem, &pt_src, 0, &blend, ULW_ALPHA);
            DeleteObject(hbmp);
            DeleteDC(hdc_mem);
            ReleaseDC(ptr::null_mut(), hdc_screen);
        }
    }

    /// Alpha-blend a pre-multiplied source DIB pixel over a destination DIB pixel.
    /// Both are in DIB BGRA layout: bytes [B,G,R,A] = little-endian u32 B|G<<8|R<<16|A<<24.
    fn alpha_blend_dib(dst: u32, src: u32) -> u32 {
        let sa = (src >> 24) & 0xFF;
        if sa == 0   { return dst; }
        if sa == 255 { return src; }
        let inv = 255 - sa;
        let b = ((src & 0xFF) * sa / 255)         + ((dst & 0xFF)         * inv / 255);
        let g = (((src >> 8)  & 0xFF) * sa / 255) + (((dst >> 8)  & 0xFF) * inv / 255);
        let r = (((src >> 16) & 0xFF) * sa / 255) + (((dst >> 16) & 0xFF) * inv / 255);
        let a = sa + ((dst >> 24) & 0xFF) * inv / 255;
        b | (g << 8) | (r << 16) | (a << 24)
    }

    // ── Window procedure ─────────────────────────────────────────────────────

    extern "system" fn wnd_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        let sp = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;

        match msg {
            // Layered windows don't receive WM_PAINT in the usual sense;
            // we drive all repainting through update_layered(). Return 0 here
            // to avoid DefWindowProc eating the message and leaving artifacts.
            0x000F /* WM_PAINT */ => 0,

            WM_MOUSEMOVE => {
                if sp.is_null() { return 0; }
                let s = unsafe { &mut *sp };
                let x = (l & 0xFFFF) as i16 as i32;
                let y = ((l >> 16) & 0xFFFF) as i16 as i32;
                let new_state = if s.hit_close(x, y) {
                    if s.btn_state == BtnState::Pressed { BtnState::Pressed } else { BtnState::Hover }
                } else {
                    if s.btn_state == BtnState::Pressed { BtnState::Pressed } else { BtnState::Normal }
                };
                if new_state != s.btn_state {
                    s.btn_state = new_state;
                    update_layered(s);
                }
                let mut tme = TRACKMOUSEEVENT {
                    cb_size: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dw_flags: TME_LEAVE, hwnd_track: hwnd, dw_hover_time: 0,
                };
                unsafe { TrackMouseEvent(&mut tme) };
                0
            }

            WM_MOUSELEAVE => {
                if !sp.is_null() {
                    let s = unsafe { &mut *sp };
                    if s.btn_state != BtnState::Normal {
                        s.btn_state = BtnState::Normal;
                        update_layered(s);
                    }
                }
                0
            }

            WM_LBUTTONDOWN => {
                if sp.is_null() { return 0; }
                let s = unsafe { &mut *sp };
                let x = (l & 0xFFFF) as i16 as i32;
                let y = ((l >> 16) & 0xFFFF) as i16 as i32;
                if s.hit_close(x, y) {
                    s.btn_state = BtnState::Pressed;
                    update_layered(s);
                }
                0
            }

            WM_LBUTTONUP => {
                if sp.is_null() { return 0; }
                let s = unsafe { &mut *sp };
                let x = (l & 0xFFFF) as i16 as i32;
                let y = ((l >> 16) & 0xFFFF) as i16 as i32;
                let was_pressed = s.btn_state == BtnState::Pressed;
                s.btn_state = if s.hit_close(x, y) { BtnState::Hover } else { BtnState::Normal };
                if was_pressed && s.hit_close(x, y) {
                    unsafe { PostQuitMessage(0) };
                } else {
                    update_layered(s);
                }
                0
            }

            // Whole window draggable; close-button area stays as HTCLIENT.
            WM_NCHITTEST => {
                let result = unsafe { DefWindowProcW(hwnd, msg, w, l) };
                if result == HTCLIENT {
                    let sx = (l & 0xFFFF) as i16 as i32;
                    let sy = ((l >> 16) & 0xFFFF) as i16 as i32;
                    let mut rect = [0i32; 4];
                    unsafe { GetWindowRect(hwnd, &mut rect) };
                    let cx = sx - rect[0];
                    let cy = sy - rect[1];
                    if cx >= BTN_X && cx < BTN_X + BTN_W && cy >= BTN_Y && cy < BTN_Y + BTN_H {
                        return HTCLIENT;
                    }
                    return HTCAPTION;
                }
                result
            }

            WM_SETCURSOR => {
                let cur = unsafe { LoadCursorW(ptr::null_mut(), IDC_ARROW) };
                unsafe { SetCursor(cur) };
                1
            }

            WM_DESTROY => {
                if !sp.is_null() {
                    unsafe {
                        drop(Box::from_raw(sp));
                        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    }
                }
                unsafe { PostQuitMessage(0) };
                0
            }

            _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
        }
    }

    // ── Entry point ──────────────────────────────────────────────────────────
    fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }

    pub fn run() -> anyhow::Result<()> {
        let hinstance = unsafe { GetModuleHandleW(ptr::null()) };
        let class_name = wide("cmsdl_gui_test");

        let wc = WNDCLASSEXW {
            cb_size:         std::mem::size_of::<WNDCLASSEXW>() as u32,
            style:           0,
            lpfn_wnd_proc:   wnd_proc,
            cb_cls_extra:    0, cb_wnd_extra: 0,
            h_instance:      hinstance,
            h_icon:          ptr::null_mut(),
            h_cursor:        unsafe { LoadCursorW(ptr::null_mut(), IDC_ARROW) },
            hbr_background:  ptr::null_mut(),
            lpsz_menu_name:  ptr::null(),
            lpsz_class_name: class_name.as_ptr(),
            h_icon_sm:       ptr::null_mut(),
        };
        if unsafe { RegisterClassExW(&wc) } == 0 {
            anyhow::bail!("RegisterClassExW failed");
        }

        let sm_cx = unsafe { GetSystemMetrics(0) };
        let sm_cy = unsafe { GetSystemMetrics(1) };
        let win_x = (sm_cx - WIN_W) / 2;
        let win_y = (sm_cy - WIN_H) / 2;

        // WS_EX_LAYERED enables per-pixel alpha via UpdateLayeredWindow.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_APPWINDOW | WS_EX_LAYERED,
                class_name.as_ptr(),
                wide("cmsdl gui test").as_ptr(),
                WS_POPUP,                      // not WS_VISIBLE yet
                win_x, win_y, WIN_W, WIN_H,
                ptr::null_mut(), ptr::null_mut(), hinstance, ptr::null_mut(),
            )
        };
        if hwnd.is_null() { anyhow::bail!("CreateWindowExW failed"); }

        // Load all pixel data.
        let bg_pixels    = load_pixels(BG_PNG, "png");
        let btn_normal   = load_pixels(BMP_NORMAL,   "bmp");
        let btn_hover    = load_pixels(BMP_HOVER,    "bmp");
        let btn_click    = load_pixels(BMP_CLICK,    "bmp");
        let prog_pixels  = load_pixels(BMP_PROGRESS, "bmp");

        let state = Box::new(WindowState {
            hwnd,
            win_x, win_y,
            bg_pixels,
            btn_pixels: [btn_normal, btn_hover, btn_click],
            prog_pixels,
            btn_state: BtnState::Normal,
            progress:  50,
        });

        // Attach state.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize) };

        // First paint + show. We must call UpdateLayeredWindow before
        // ShowWindow so the window surface is ready when it becomes visible.
        let sp = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
        if !sp.is_null() {
            update_layered(unsafe { &*sp });
        }

        // Show the window (SW_SHOW = 5).
        extern "system" { fn ShowWindow(hwnd: HWND, cmd: i32) -> BOOL; }
        unsafe { ShowWindow(hwnd, 5) };

        // Message loop.
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        loop {
            let ret = unsafe { GetMessageW(&mut msg, ptr::null_mut(), 0, 0) };
            if ret <= 0 { break; }
            unsafe { TranslateMessage(&msg); DispatchMessageW(&msg); }
        }

        Ok(())
    }
} // mod win32
