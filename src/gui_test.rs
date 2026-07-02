/// GUI test window - per-pixel alpha via UpdateLayeredWindow.
///
/// Window body:   src/gui_res/139.png  (626 x 583, has alpha/rounded corners)
/// Close button:  150.bmp normal  151.bmp hover  152.bmp click  (24 x 24 each)
/// Progress bar:  154.bmp  (63 x 12, tiled horizontally)
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
    const ULW_ALPHA:      u32 = 0x0002;
    const AC_SRC_OVER:    u8  = 0x00;
    const AC_SRC_ALPHA:   u8  = 0x01;
    const BI_RGB:         u32 = 0;
    const DIB_RGB_COLORS: u32 = 0;

    // Window styles
    const WS_POPUP:        u32 = 0x80000000;
    const WS_EX_APPWINDOW: u32 = 0x00040000;
    const WS_EX_LAYERED:   u32 = 0x00080000;

    const GWLP_USERDATA: i32   = -21;
    const IDC_ARROW:     usize = 32512;

    // GDI+ pixel format
    const PIXEL_FORMAT_32BPP_ARGB: i32 = 0x26200A;
    const IMAGING_LOCK_READ:       u32 = 1;

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

    #[repr(C)]
    struct BLENDFUNCTION {
        blend_op:              u8,
        blend_flags:           u8,
        source_constant_alpha: u8,
        alpha_format:          u8,
    }

    #[repr(C)]
    struct POINT { x: i32, y: i32 }

    #[repr(C)]
    struct SIZE { cx: i32, cy: i32 }

    #[repr(C)]
    struct BITMAPINFOHEADER {
        bi_size: u32, bi_width: i32, bi_height: i32,
        bi_planes: u16, bi_bit_count: u16, bi_compression: u32,
        bi_size_image: u32, bi_x_pels_per_meter: i32, bi_y_pels_per_meter: i32,
        bi_clr_used: u32, bi_clr_important: u32,
    }

    #[repr(C)]
    struct BITMAPINFO {
        bmi_header: BITMAPINFOHEADER,
        bmi_colors: [u32; 1],
    }

    #[repr(C)]
    struct GdiplusStartupInput {
        version: u32, debug_event_callback: *const c_void,
        suppress_background_thread: BOOL, suppress_external_codecs: BOOL,
    }

    #[repr(C)]
    struct BitmapData {
        width: u32, height: u32, stride: i32,
        pixel_format: i32, scan0: *mut u8, reserved: usize,
    }

    // ── FFI imports ──────────────────────────────────────────────────────────
    #[allow(dead_code)]
    #[link(name = "user32")]
    extern "system" {
        fn RegisterClassExW(wc: *const WNDCLASSEXW) -> ATOM;
        fn CreateWindowExW(ex: u32, cls: *const u16, name: *const u16,
            style: u32, x: i32, y: i32, w: i32, h: i32,
            parent: HWND, menu: HMENU, inst: HINSTANCE, param: *mut c_void) -> HWND;
        fn ShowWindow(hwnd: HWND, cmd: i32) -> BOOL;
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

    #[allow(dead_code)]
    #[link(name = "gdi32")]
    extern "system" {
        fn GetDC(hwnd: HWND) -> HDC;
        fn ReleaseDC(hwnd: HWND, hdc: HDC) -> i32;
        fn CreateCompatibleDC(hdc: HDC) -> HDC;
        fn SelectObject(hdc: HDC, obj: HGDIOBJ) -> HGDIOBJ;
        fn DeleteDC(hdc: HDC) -> BOOL;
        fn DeleteObject(obj: HGDIOBJ) -> BOOL;
        fn CreateDIBSection(hdc: HDC, bmi: *const BITMAPINFO, usage: u32,
            bits: *mut *mut u8, section: *mut c_void, offset: u32) -> HBMP;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> HINSTANCE;
    }

    extern "system" {
        fn GdiplusStartup(token: *mut usize, input: *const GdiplusStartupInput,
            output: *mut c_void) -> i32;
        fn GdipCreateBitmapFromFile(file: *const u16, bmp: *mut *mut c_void) -> i32;
        fn GdipBitmapLockBits(bmp: *mut c_void, rect: *const [i32; 4], flags: u32,
            format: i32, data: *mut BitmapData) -> i32;
        fn GdipBitmapUnlockBits(bmp: *mut c_void, data: *mut BitmapData) -> i32;
        fn GdipDisposeImage(img: *mut c_void) -> i32;
        fn GdipGetImageWidth(img: *mut c_void, w: *mut u32) -> i32;
        fn GdipGetImageHeight(img: *mut c_void, h: *mut u32) -> i32;
    }

    // ── State ────────────────────────────────────────────────────────────────
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum BtnState { Normal, Hover, Pressed }

    struct WindowState {
        hwnd:          HWND,
        bg_pixels:     Vec<u32>,      // WIN_W * WIN_H, pre-multiplied ARGB
        btn_pixels:    [Vec<u32>; 3], // Normal / Hover / Pressed (BTN_W*BTN_H each)
        prog_pixels:   Vec<u32>,      // PROG_TILE_W * PROG_H
        btn_state:     BtnState,
        progress:      u32,           // 0-100
    }

    impl WindowState {
        fn hit_close(&self, x: i32, y: i32) -> bool {
            x >= BTN_X && x < BTN_X + BTN_W && y >= BTN_Y && y < BTN_Y + BTN_H
        }
    }

    // ── GDI+ pixel loader ────────────────────────────────────────────────────

    static GDIP_INITED: std::sync::Once = std::sync::Once::new();

    fn gdip_init() {
        GDIP_INITED.call_once(|| {
            let input = GdiplusStartupInput {
                version: 1, debug_event_callback: ptr::null(),
                suppress_background_thread: 0, suppress_external_codecs: 0,
            };
            let mut token: usize = 0;
            unsafe { GdiplusStartup(&mut token, &input, ptr::null_mut()) };
        });
    }

    /// Load image bytes -> pre-multiplied ARGB pixel Vec (top-down, row-major).
    fn load_pixels(data: &[u8], ext: &str) -> Vec<u32> {
        gdip_init();
        let tmp = {
            let mut p = std::env::temp_dir();
            p.push(format!("cmsdl_gui_{:x}.{ext}", data.as_ptr() as usize));
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
        unsafe { GdipGetImageWidth(img, &mut w); GdipGetImageHeight(img, &mut h); }

        let mut bd = BitmapData { width: 0, height: 0, stride: 0,
            pixel_format: 0, scan0: ptr::null_mut(), reserved: 0 };
        let rect = [0i32, 0, w as i32, h as i32];
        let ok = unsafe {
            GdipBitmapLockBits(img, &rect, IMAGING_LOCK_READ, PIXEL_FORMAT_32BPP_ARGB, &mut bd)
        };
        if ok != 0 { unsafe { GdipDisposeImage(img) }; return Vec::new(); }

        let mut pixels = Vec::with_capacity((w * h) as usize);
        for row in 0..h as i32 {
            let row_ptr = unsafe { bd.scan0.offset((row * bd.stride) as isize) as *const u32 };
            for col in 0..w {
                // GDI+ ARGB: 0xAARRGGBB — pre-multiply RGB by alpha
                let argb = unsafe { *row_ptr.add(col as usize) };
                let a = (argb >> 24) & 0xFF;
                let pixel = if a == 0xFF {
                    argb
                } else {
                    let r = (((argb >> 16) & 0xFF) * a / 255) & 0xFF;
                    let g = (((argb >>  8) & 0xFF) * a / 255) & 0xFF;
                    let b = (( argb        & 0xFF) * a / 255) & 0xFF;
                    (a << 24) | (r << 16) | (g << 8) | b
                };
                pixels.push(pixel);
            }
        }
        unsafe { GdipBitmapUnlockBits(img, &mut bd); GdipDisposeImage(img); }
        pixels
    }

    // ── Compositing ──────────────────────────────────────────────────────────

    /// Rebuild the composited surface and call UpdateLayeredWindow.
    ///
    /// `pt_dst`: pass `Some(pos)` only for the initial placement.
    /// For all repaints triggered by hover/click state changes pass `None`,
    /// which keeps the window at whatever screen position it is currently at
    /// (i.e. wherever the user dragged it).
    fn update_layered(s: &WindowState, pt_dst: Option<POINT>) {
        let bmi = BITMAPINFO {
            bmi_header: BITMAPINFOHEADER {
                bi_size:             std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                bi_width:            WIN_W,
                bi_height:           -WIN_H,  // negative = top-down
                bi_planes:           1,
                bi_bit_count:        32,
                bi_compression:      BI_RGB,
                bi_size_image:       0,
                bi_x_pels_per_meter: 0, bi_y_pels_per_meter: 0,
                bi_clr_used:         0, bi_clr_important:    0,
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

        // DIB pixel buffer: u32 per pixel, stored as bytes [B, G, R, A]
        // (little-endian: 0xAARRGGBB ARGB -> write as B|G<<8|R<<16|A<<24)
        let dib = unsafe {
            std::slice::from_raw_parts_mut(dib_bits as *mut u32, (WIN_W * WIN_H) as usize)
        };

        // ARGB (0xAARRGGBB) -> DIB BGRA dword
        let to_dib = |argb: u32| -> u32 {
            let a = (argb >> 24) & 0xFF;
            let r = (argb >> 16) & 0xFF;
            let g = (argb >>  8) & 0xFF;
            let b =  argb        & 0xFF;
            b | (g << 8) | (r << 16) | (a << 24)
        };

        // 1. Background
        for (i, &px) in s.bg_pixels.iter().enumerate().take(dib.len()) {
            dib[i] = to_dib(px);
        }

        // 2. Close button — alpha-blend over background
        let btn_px = &s.btn_pixels[match s.btn_state {
            BtnState::Normal  => 0,
            BtnState::Hover   => 1,
            BtnState::Pressed => 2,
        }];
        for by in 0..BTN_H {
            for bx in 0..BTN_W {
                let dx = BTN_X + bx;
                let dy = BTN_Y + by;
                if dx < 0 || dx >= WIN_W || dy < 0 || dy >= WIN_H { continue; }
                let si = (by * BTN_W + bx) as usize;
                if si >= btn_px.len() { continue; }
                let di = (dy * WIN_W + dx) as usize;
                dib[di] = alpha_blend_dib(dib[di], to_dib(btn_px[si]));
            }
        }

        // 3. Progress bar — tile and blend
        let filled = (PROG_RENDERED_W * s.progress.min(100) as i32) / 100;
        for py in 0..PROG_H {
            for px in 0..filled {
                let dx = PROG_X + px;
                let dy = PROG_Y + py;
                if dx < 0 || dx >= WIN_W || dy < 0 || dy >= WIN_H { continue; }
                let si = (py * PROG_TILE_W + (px % PROG_TILE_W)) as usize;
                if si >= s.prog_pixels.len() { continue; }
                let di = (dy * WIN_W + dx) as usize;
                dib[di] = alpha_blend_dib(dib[di], to_dib(s.prog_pixels[si]));
            }
        }

        // Call UpdateLayeredWindow.
        // pt_dst = NULL -> keep current screen position (don't snap after drag).
        let pt_ptr = match pt_dst { Some(ref p) => p as *const POINT, None => ptr::null() };
        let sz    = SIZE  { cx: WIN_W, cy: WIN_H };
        let pt_src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            blend_op: AC_SRC_OVER, blend_flags: 0,
            source_constant_alpha: 255, alpha_format: AC_SRC_ALPHA,
        };
        unsafe {
            UpdateLayeredWindow(s.hwnd, hdc_screen, pt_ptr, &sz,
                hdc_mem, &pt_src, 0, &blend, ULW_ALPHA);
            DeleteObject(hbmp);
            DeleteDC(hdc_mem);
            ReleaseDC(ptr::null_mut(), hdc_screen);
        }
    }

    /// Alpha-blend a pre-multiplied source DIB pixel over a destination DIB pixel.
    /// Both pixels use DIB BGRA layout: bytes [B,G,R,A] = u32 B|G<<8|R<<16|A<<24.
    fn alpha_blend_dib(dst: u32, src: u32) -> u32 {
        let sa = (src >> 24) & 0xFF;
        if sa == 0   { return dst; }
        if sa == 255 { return src; }
        let inv = 255 - sa;
        let b = (( src        & 0xFF) * sa / 255) + (( dst        & 0xFF) * inv / 255);
        let g = (((src >>  8) & 0xFF) * sa / 255) + (((dst >>  8) & 0xFF) * inv / 255);
        let r = (((src >> 16) & 0xFF) * sa / 255) + (((dst >> 16) & 0xFF) * inv / 255);
        let a = sa + ((dst >> 24) & 0xFF) * inv / 255;
        b | (g << 8) | (r << 16) | (a << 24)
    }

    // ── Window procedure ─────────────────────────────────────────────────────

    extern "system" fn wnd_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        let sp = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;

        match msg {
            // Layered windows don't use WM_PAINT; all drawing goes through
            // update_layered(). Return 0 to suppress default handling.
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
                    // None = keep current screen position
                    update_layered(s, None);
                }
                // Request WM_MOUSELEAVE when the cursor exits the window.
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
                        update_layered(s, None);
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
                    update_layered(s, None);
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
                    update_layered(s, None);
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

        // Centre window on primary monitor.
        let sm_cx = unsafe { GetSystemMetrics(0) };
        let sm_cy = unsafe { GetSystemMetrics(1) };
        let win_x = (sm_cx - WIN_W) / 2;
        let win_y = (sm_cy - WIN_H) / 2;

        // WS_EX_LAYERED enables per-pixel alpha via UpdateLayeredWindow.
        // Do NOT set WS_VISIBLE yet — show after first UpdateLayeredWindow call.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_APPWINDOW | WS_EX_LAYERED,
                class_name.as_ptr(),
                wide("cmsdl gui test").as_ptr(),
                WS_POPUP,
                win_x, win_y, WIN_W, WIN_H,
                ptr::null_mut(), ptr::null_mut(), hinstance, ptr::null_mut(),
            )
        };
        if hwnd.is_null() { anyhow::bail!("CreateWindowExW failed"); }

        // Load pixel data for all resources.
        let state = Box::new(WindowState {
            hwnd,
            bg_pixels:   load_pixels(BG_PNG,       "png"),
            btn_pixels:  [
                load_pixels(BMP_NORMAL,   "bmp"),
                load_pixels(BMP_HOVER,    "bmp"),
                load_pixels(BMP_CLICK,    "bmp"),
            ],
            prog_pixels: load_pixels(BMP_PROGRESS, "bmp"),
            btn_state:   BtnState::Normal,
            progress:    50,
        });

        // Attach state to window.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize) };

        // Initial paint with explicit screen position, then show.
        let sp = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
        if !sp.is_null() {
            update_layered(unsafe { &*sp }, Some(POINT { x: win_x, y: win_y }));
        }
        unsafe { ShowWindow(hwnd, 5 /* SW_SHOW */) };

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
