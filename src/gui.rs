/// GUI patcher window - per-pixel alpha via UpdateLayeredWindow.
///
/// Window body:   src/gui_res/139.png  (626 x 583, has alpha/rounded corners)
/// Close button:  150.bmp normal  151.bmp hover  152.bmp click  (24 x 24 each)
/// Progress bar:  154A-154F.bmp, a two-layer track + fill bar (12 px tall),
///                drawn at offset (13, 556) with a total rendered length of
///                600px:
///                  Track (bottom layer, always fully visible, left to
///                  right): A (left cap) + E tiled (repeatable empty body)
///                  + F (right cap). E is tiled/clipped to fill exactly
///                  whatever width remains after A and F, so the track
///                  always spans the full 600px.
///                  Fill (top layer, grows with progress): B (start cap) +
///                  C tiled (body) + D (end cap). The fill starts at the
///                  same x position as A and grows until it fully covers
///                  the entire track (A + E + F).
///                All slice widths are read from the loaded bitmaps at
///                runtime rather than hardcoded, so this stays correct if
///                the source art is resized.
/// Status labels: both use SimSun 9pt.
///                  Percentage label at (12, 540), color #979497.
///                  Status message label at (13, 520), color #66B2FF.
///                Both reflect the current demo animation progress.
///
/// WS_EX_LAYERED + UpdateLayeredWindow(ULW_ALPHA) gives us per-pixel
/// transparency so the rounded corners show through to the desktop.

use std::sync::{Arc, Mutex};

/// Shared UI model driven by whatever task owns the window (the demo animation
/// or the real patcher). The window's repaint timer reads this on each tick.
#[derive(Default)]
pub struct UiModel {
    /// Primary status line (label1), drawn at (12, 540) in #979497.
    pub label1: String,
    /// Secondary status line (label2), drawn at (13, 520) in #66B2FF.
    pub label2: String,
    /// ETA label (label3), right-aligned to the progress bar, same Y as
    /// label2, drawn in #66B2FF.
    pub label3: String,
    /// Progress-bar fill ratio, 0.0..=1.0.
    pub progress: f32,
    /// When set, the window closes itself on the next timer tick.
    pub should_close: bool,
}

impl UiModel {
    pub fn new() -> Arc<Mutex<UiModel>> {
        Arc::new(Mutex::new(UiModel::default()))
    }
}

/// Open the patcher window and run its message loop until closed. The window
/// renders entirely from `ui`; a background task is expected to update `ui`
/// concurrently. Returns once the window is closed.
pub fn run_window(ui: Arc<Mutex<UiModel>>) -> anyhow::Result<()> {
    #[cfg(windows)]
    { win32::run_window(ui) }
    #[cfg(not(windows))]
    { let _ = ui; anyhow::bail!("the GUI is only supported on Windows") }
}

/// Detach from and destroy the console window when it was allocated for this
/// process (i.e. we were launched from Explorer or a shortcut, not from an
/// existing terminal).
///
/// cmsdl is a console-subsystem program, so a fresh console pops up when it is
/// double-clicked. In GUI mode that window is just empty noise. We detect
/// ownership via `GetConsoleProcessList`: if we are the only process attached,
/// the console is ours to destroy; if a shell shares it, we leave it alone so
/// the user's terminal is untouched.
///
/// We use `FreeConsole` rather than `ShowWindow(SW_HIDE)` because the latter
/// only hides the window *after* it has already been painted, causing a visible
/// flash. `FreeConsole` detaches from the console immediately, destroying the
/// window without any flicker.
#[cfg(windows)]
pub(crate) fn hide_own_console() {
    extern "system" {
        fn GetConsoleProcessList(process_list: *mut u32, count: u32) -> u32;
        fn FreeConsole() -> i32;
    }
    // SAFETY: both calls take well-defined arguments and are always safe.
    unsafe {
        let mut pids = [0u32; 2];
        let count = GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32);
        if count <= 1 {
            FreeConsole();
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn hide_own_console() {}



#[cfg(windows)]
mod win32 {
    use std::ffi::c_void;
    use std::ptr;

    // ── Embedded resources ───────────────────────────────────────────────────
    const BG_PNG: &[u8]       = include_bytes!("gui_res/139.png");
    const BMP_NORMAL: &[u8]   = include_bytes!("gui_res/150.bmp");
    const BMP_HOVER: &[u8]    = include_bytes!("gui_res/151.bmp");
    const BMP_CLICK: &[u8]    = include_bytes!("gui_res/152.bmp");
    const BMP_MIN_NORMAL: &[u8] = include_bytes!("gui_res/146.bmp"); // minimize: normal
    const BMP_MIN_HOVER: &[u8]  = include_bytes!("gui_res/147.bmp"); // minimize: hover
    const BMP_MIN_CLICK: &[u8]  = include_bytes!("gui_res/148.bmp"); // minimize: pressed
    const BMP_PROG_A: &[u8]   = include_bytes!("gui_res/154A.bmp"); // track: left cap
    const BMP_PROG_B: &[u8]   = include_bytes!("gui_res/154B.bmp"); // fill: start cap
    const BMP_PROG_C: &[u8]   = include_bytes!("gui_res/154C.bmp"); // fill: repeatable body
    const BMP_PROG_D: &[u8]   = include_bytes!("gui_res/154D.bmp"); // fill: end cap
    const BMP_PROG_E: &[u8]   = include_bytes!("gui_res/154E.bmp"); // track: repeatable empty body
    const BMP_PROG_F: &[u8]   = include_bytes!("gui_res/154F.bmp"); // track: right cap

    // Dynamic taskbar progress icon: a background plate with a percentage
    // number (0..=100) composited on top.
    const PROGRESS_BG_ICON: &[u8] = include_bytes!("gui_res/progress_background_icon.png");

    macro_rules! number_png {
        ($n:literal) => {
            include_bytes!(concat!("gui_res/numbered_images/", stringify!($n), ".png")) as &[u8]
        };
    }
    /// Foreground number overlays indexed by percentage (0..=100).
    const NUMBER_PNGS: [&[u8]; 101] = [
        number_png!(0),   number_png!(1),   number_png!(2),   number_png!(3),   number_png!(4),
        number_png!(5),   number_png!(6),   number_png!(7),   number_png!(8),   number_png!(9),
        number_png!(10),  number_png!(11),  number_png!(12),  number_png!(13),  number_png!(14),
        number_png!(15),  number_png!(16),  number_png!(17),  number_png!(18),  number_png!(19),
        number_png!(20),  number_png!(21),  number_png!(22),  number_png!(23),  number_png!(24),
        number_png!(25),  number_png!(26),  number_png!(27),  number_png!(28),  number_png!(29),
        number_png!(30),  number_png!(31),  number_png!(32),  number_png!(33),  number_png!(34),
        number_png!(35),  number_png!(36),  number_png!(37),  number_png!(38),  number_png!(39),
        number_png!(40),  number_png!(41),  number_png!(42),  number_png!(43),  number_png!(44),
        number_png!(45),  number_png!(46),  number_png!(47),  number_png!(48),  number_png!(49),
        number_png!(50),  number_png!(51),  number_png!(52),  number_png!(53),  number_png!(54),
        number_png!(55),  number_png!(56),  number_png!(57),  number_png!(58),  number_png!(59),
        number_png!(60),  number_png!(61),  number_png!(62),  number_png!(63),  number_png!(64),
        number_png!(65),  number_png!(66),  number_png!(67),  number_png!(68),  number_png!(69),
        number_png!(70),  number_png!(71),  number_png!(72),  number_png!(73),  number_png!(74),
        number_png!(75),  number_png!(76),  number_png!(77),  number_png!(78),  number_png!(79),
        number_png!(80),  number_png!(81),  number_png!(82),  number_png!(83),  number_png!(84),
        number_png!(85),  number_png!(86),  number_png!(87),  number_png!(88),  number_png!(89),
        number_png!(90),  number_png!(91),  number_png!(92),  number_png!(93),  number_png!(94),
        number_png!(95),  number_png!(96),  number_png!(97),  number_png!(98),  number_png!(99),
        number_png!(100),
    ];

    // ── Layout ───────────────────────────────────────────────────────────────
    const WIN_W: i32 = 626;
    const WIN_H: i32 = 583;
    const BTN_W: i32 = 24;
    const BTN_H: i32 = 24;
    const BTN_X: i32 = WIN_W - BTN_W - 8;
    const BTN_Y: i32 = 8;
    /// Gap between the minimize and close buttons.
    const BTN_GAP: i32 = 0;
    /// Minimize button sits immediately to the left of the close button.
    const MIN_BTN_X: i32 = BTN_X - BTN_W - BTN_GAP;
    const MIN_BTN_Y: i32 = BTN_Y;
    const PROG_H: i32 = 12;
    const PROG_X: i32 = 13;
    const PROG_Y: i32 = 556;
    /// Total rendered length of the progress bar, in pixels.
    const PROG_TOTAL_W: i32 = 600;

    // Status label (percentage, below the progress bar).
    const LABEL_X: i32 = 12;
    const LABEL_Y: i32 = 540;
    const LABEL_FONT_NAME: &str = "SimSun";
    const LABEL_FONT_PT: i32 = 9;
    const LABEL_COLOR_R: u8 = 0x97;
    const LABEL_COLOR_G: u8 = 0x94;
    const LABEL_COLOR_B: u8 = 0x97;

    // Status message label (above the percentage label).
    const LABEL2_X: i32 = 13;
    const LABEL2_Y: i32 = 520;
    const LABEL2_COLOR_R: u8 = 0x66;
    const LABEL2_COLOR_G: u8 = 0xB2;
    const LABEL2_COLOR_B: u8 = 0xFF;

    // ETA label (right-aligned to the progress bar, same row as label2).
    const LABEL3_RIGHT_X: i32 = PROG_X + PROG_TOTAL_W;
    const LABEL3_Y: i32 = 520;
    const LABEL3_COLOR_R: u8 = 0x66;
    const LABEL3_COLOR_G: u8 = 0xB2;
    const LABEL3_COLOR_B: u8 = 0xFF;

    // cmsdl version label (top area). Same style as label1 (#979497).
    const LABEL_VER_X: i32 = 343;
    const LABEL_VER_Y: i32 = 18;
    const LABEL_VER_TEXT: &str = concat!("v", env!("CARGO_PKG_VERSION"));

    // Repaint timer.
    const ANIM_TICK_MS:   u32 = 33;      // ~30 fps repaint tick
    const IDT_ANIM:       usize = 1;     // timer id

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
    const WM_TIMER:       u32 = 0x0113;
    const WM_SETICON:     u32 = 0x0080;
    const ICON_SMALL:     usize = 0;
    const ICON_BIG:       usize = 1;
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
    const WS_SYSMENU:      u32 = 0x00080000;
    const WS_MINIMIZEBOX:  u32 = 0x00020000;
    const WS_EX_APPWINDOW: u32 = 0x00040000;
    const WS_EX_LAYERED:   u32 = 0x00080000;

    const GWLP_USERDATA: i32   = -21;
    const IDC_ARROW:     usize = 32512;

    // ShowWindow commands
    const SW_MINIMIZE: i32 = 6;

    // GDI+ pixel format
    const PIXEL_FORMAT_32BPP_ARGB: i32 = 0x26200A;
    const IMAGING_LOCK_READ:       u32 = 1;

    // Font creation
    const FW_NORMAL:      i32 = 400;
    const DEFAULT_CHARSET: u32 = 1;
    const OUT_DEFAULT_PRECIS: u32 = 0;
    const CLIP_DEFAULT_PRECIS: u32 = 0;
    const ANTIALIASED_QUALITY: u32 = 4;
    const DEFAULT_PITCH: u32 = 0;
    const FF_DONTCARE:   u32 = 0;
    const TRANSPARENT_BKMODE: i32 = 1;

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
    struct ICONINFO {
        f_icon:    BOOL,
        x_hotspot: u32,
        y_hotspot: u32,
        hbm_mask:  HBMP,
        hbm_color: HBMP,
    }

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
        fn SetTimer(hwnd: HWND, id: usize, elapse: u32, proc: *const c_void) -> usize;
        fn KillTimer(hwnd: HWND, id: usize) -> BOOL;
        fn GetTickCount() -> u32;
        fn SetProcessDPIAware() -> BOOL;
        fn UpdateLayeredWindow(hwnd: HWND, hdc_dst: HDC, pt_dst: *const POINT,
            size: *const SIZE, hdc_src: HDC, pt_src: *const POINT,
            key: u32, blend: *const BLENDFUNCTION, flags: u32) -> BOOL;
        fn SendMessageW(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT;
        fn CreateIconIndirect(info: *const ICONINFO) -> HICON;
        fn DestroyIcon(icon: HICON) -> BOOL;
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
        fn CreateBitmap(w: i32, h: i32, planes: u32, bit_count: u32, bits: *const c_void) -> HBMP;
        fn CreateFontW(height: i32, width: i32, escapement: i32, orientation: i32,
            weight: i32, italic: u32, underline: u32, strikeout: u32,
            char_set: u32, out_precision: u32, clip_precision: u32,
            quality: u32, pitch_and_family: u32, face_name: *const u16) -> HGDIOBJ;
        fn SetTextColor(hdc: HDC, color: u32) -> u32;
        fn SetBkMode(hdc: HDC, mode: i32) -> i32;
        fn SetBkColor(hdc: HDC, color: u32) -> u32;
        fn TextOutW(hdc: HDC, x: i32, y: i32, text: *const u16, len: i32) -> BOOL;
        fn GetDeviceCaps(hdc: HDC, index: i32) -> i32;
        fn GetTextExtentPoint32W(hdc: HDC, text: *const u16, len: i32, size: *mut SIZE) -> BOOL;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> HINSTANCE;
        fn GetProcAddress(module: HINSTANCE, name: *const u8) -> *const c_void;
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
        btn_pixels:    [Vec<u32>; 3], // close: Normal / Hover / Pressed (BTN_W*BTN_H each)
        min_btn_pixels: [Vec<u32>; 3], // minimize: Normal / Hover / Pressed

        /// Track slices (always fully drawn): left cap A, repeatable empty
        /// body E, right cap F. Each is a (pixels, width) pair.
        track_a: (Vec<u32>, i32),
        track_f: (Vec<u32>, i32),
        track_e: (Vec<u32>, i32),
        /// Fill slices (grow with progress): start cap B, repeatable body C,
        /// end cap D.
        fill_b: (Vec<u32>, i32),
        fill_c: (Vec<u32>, i32),
        fill_d: (Vec<u32>, i32),

        btn_state:     BtnState,
        min_btn_state: BtnState,

        /// Font used to draw the status labels (SimSun, 9pt).
        label_font:    HGDIOBJ,

        /// Shared UI model (label text + progress), updated by the owning task.
        ui:            super::Arc<super::Mutex<super::UiModel>>,

        /// Background plate for the dynamic taskbar icon (straight ARGB + size).
        icon_bg:       (Vec<u32>, i32, i32),
        /// Last percentage (0..=100) rendered into the taskbar icon; -1 = none.
        icon_pct:      i32,
        /// Current taskbar icon handle, owned by us (destroyed on replace/drop).
        icon:          HICON,

        /// Windows taskbar progress interface, when available. Mirrors the
        /// window's progress-bar fill onto its taskbar button so progress is
        /// visible even when the window is minimised.
        taskbar:       Option<windows::Win32::UI::Shell::ITaskbarList3>,
    }

    impl WindowState {
        fn hit_close(&self, x: i32, y: i32) -> bool {
            x >= BTN_X && x < BTN_X + BTN_W && y >= BTN_Y && y < BTN_Y + BTN_H
        }

        fn hit_min(&self, x: i32, y: i32) -> bool {
            x >= MIN_BTN_X && x < MIN_BTN_X + BTN_W && y >= MIN_BTN_Y && y < MIN_BTN_Y + BTN_H
        }

        /// Snapshot the current label text and progress from the shared model.
        fn snapshot(&self) -> (String, String, String, f32, bool) {
            match self.ui.lock() {
                Ok(m) => (m.label1.clone(), m.label2.clone(), m.label3.clone(), m.progress.clamp(0.0, 1.0), m.should_close),
                Err(_) => (String::new(), String::new(), String::new(), 0.0, false),
            }
        }

        /// Rebuild the taskbar icon to show the current percentage, composited
        /// over the background plate. A no-op when the integer percentage has
        /// not changed since the last update, so this is cheap to call on every
        /// repaint tick.
        fn update_taskbar_icon(&mut self, ratio: f32) {
            let pct = (ratio.clamp(0.0, 1.0) * 100.0).round() as i32;
            if pct == self.icon_pct {
                return;
            }

            let (ref bg, bw, bh) = self.icon_bg;
            if bg.is_empty() || bw <= 0 || bh <= 0 {
                return;
            }
            let (fg, fw, fh) = load_argb(NUMBER_PNGS[pct.clamp(0, 100) as usize], "png");
            if fw != bw || fh != bh {
                return;
            }

            let composed = composite_straight(bg, &fg, bw, bh);
            let hicon = create_alpha_icon(&composed, bw, bh);
            if hicon.is_null() {
                return;
            }

            // Point the window (and therefore its taskbar button) at the new icon.
            unsafe {
                SendMessageW(self.hwnd, WM_SETICON, ICON_BIG, hicon as LPARAM);
                SendMessageW(self.hwnd, WM_SETICON, ICON_SMALL, hicon as LPARAM);
            }
            // Now safe to free the previously-set icon.
            if !self.icon.is_null() {
                unsafe { DestroyIcon(self.icon) };
            }
            self.icon = hicon;
            self.icon_pct = pct;
        }
    }

    impl Drop for WindowState {
        fn drop(&mut self) {
            if self.label_font != 0 {
                unsafe { DeleteObject(self.label_font) };
            }
            if !self.icon.is_null() {
                unsafe { DestroyIcon(self.icon) };
            }
        }
    }

    /// Create the taskbar-progress COM object (`ITaskbarList3`) on the current
    /// (message-loop) thread. Returns `None` when COM initialisation or object
    /// creation fails, in which case taskbar progress is simply not shown.
    ///
    /// This is what makes the progress visible on the window's taskbar button:
    /// in GUI mode the process has detached from any console (see
    /// `hide_own_console`), so the console-based taskbar reporter is inert and
    /// the window must drive its own indicator.
    fn init_taskbar() -> Option<windows::Win32::UI::Shell::ITaskbarList3> {
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
        };
        use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};

        // SAFETY: standard COM initialisation on this thread; the result is
        // ignored because COM may already be initialised with a compatible
        // apartment model.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let tb: ITaskbarList3 = CoCreateInstance(&TaskbarList, None, CLSCTX_ALL).ok()?;
            tb.HrInit().ok()?;
            Some(tb)
        }
    }

    /// Reflect `ratio` (0.0..=1.0) onto the taskbar progress button. A no-op
    /// when the taskbar interface is unavailable. Calls are cheap and ignored
    /// by Windows until the taskbar button actually exists, so it is safe to
    /// call on every repaint tick.
    fn set_taskbar_progress(s: &WindowState, ratio: f32) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::Shell::TBPF_NORMAL;

        if let Some(tb) = &s.taskbar {
            let value = (ratio.clamp(0.0, 1.0) as f64 * 1000.0).round() as u64;
            // SAFETY: `s.hwnd` is this window's valid handle for its lifetime.
            unsafe {
                let _ = tb.SetProgressState(HWND(s.hwnd), TBPF_NORMAL);
                let _ = tb.SetProgressValue(HWND(s.hwnd), value, 1000);
            }
        }
    }

    /// Create the SimSun 9pt font used for the status label.
    /// The 9pt size is converted to device pixels at a fixed 96 DPI so the
    /// text is identical at every system DPI (the process is marked DPI-aware,
    /// so Windows applies no scaling and our layout stays pixel-exact).
    fn create_label_font() -> HGDIOBJ {
        // Standard point-to-pixel conversion at 96 DPI: -(pt * 96 / 72).
        // Negative height requests a font matched by character height rather
        // than cell height.
        let height = -(LABEL_FONT_PT * 96 / 72);

        let face: Vec<u16> = LABEL_FONT_NAME.encode_utf16().chain(Some(0)).collect();
        unsafe {
            CreateFontW(
                height, 0, 0, 0, FW_NORMAL, 0, 0, 0,
                DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY, DEFAULT_PITCH | FF_DONTCARE,
                face.as_ptr(),
            )
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

    /// Load image bytes -> (pre-multiplied ARGB pixel Vec, width in pixels).
    /// Pixels are top-down, row-major.
    fn load_pixels(data: &[u8], ext: &str) -> (Vec<u32>, i32) {
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
        if ok != 0 || img.is_null() { return (Vec::new(), 0); }

        let mut w = 0u32;
        let mut h = 0u32;
        unsafe { GdipGetImageWidth(img, &mut w); GdipGetImageHeight(img, &mut h); }

        let mut bd = BitmapData { width: 0, height: 0, stride: 0,
            pixel_format: 0, scan0: ptr::null_mut(), reserved: 0 };
        let rect = [0i32, 0, w as i32, h as i32];
        let ok = unsafe {
            GdipBitmapLockBits(img, &rect, IMAGING_LOCK_READ, PIXEL_FORMAT_32BPP_ARGB, &mut bd)
        };
        if ok != 0 { unsafe { GdipDisposeImage(img) }; return (Vec::new(), 0); }

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
        (pixels, w as i32)
    }

    /// Load image bytes -> (straight/non-premultiplied ARGB `0xAARRGGBB`, width,
    /// height). Unlike [`load_pixels`], the RGB channels are left as-is (not
    /// multiplied by alpha), which is the format Windows expects for the color
    /// bitmap of an alpha icon created via `CreateIconIndirect`.
    fn load_argb(data: &[u8], ext: &str) -> (Vec<u32>, i32, i32) {
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
        if ok != 0 || img.is_null() { return (Vec::new(), 0, 0); }

        let mut w = 0u32;
        let mut h = 0u32;
        unsafe { GdipGetImageWidth(img, &mut w); GdipGetImageHeight(img, &mut h); }

        let mut bd = BitmapData { width: 0, height: 0, stride: 0,
            pixel_format: 0, scan0: ptr::null_mut(), reserved: 0 };
        let rect = [0i32, 0, w as i32, h as i32];
        // PIXEL_FORMAT_32BPP_ARGB is non-premultiplied, exactly what we want.
        let ok = unsafe {
            GdipBitmapLockBits(img, &rect, IMAGING_LOCK_READ, PIXEL_FORMAT_32BPP_ARGB, &mut bd)
        };
        if ok != 0 { unsafe { GdipDisposeImage(img) }; return (Vec::new(), 0, 0); }

        let mut pixels = Vec::with_capacity((w * h) as usize);
        for row in 0..h as i32 {
            let row_ptr = unsafe { bd.scan0.offset((row * bd.stride) as isize) as *const u32 };
            for col in 0..w {
                pixels.push(unsafe { *row_ptr.add(col as usize) });
            }
        }
        unsafe { GdipBitmapUnlockBits(img, &mut bd); GdipDisposeImage(img); }
        (pixels, w as i32, h as i32)
    }

    /// Composite `fg` over `bg` (both straight ARGB `0xAARRGGBB`, same size)
    /// using the standard "source-over" operator, returning straight ARGB.
    fn composite_straight(bg: &[u32], fg: &[u32], w: i32, h: i32) -> Vec<u32> {
        let n = (w * h) as usize;
        let mut out = vec![0u32; n];
        let limit = n.min(bg.len()).min(fg.len());
        for i in 0..limit {
            let (fa, fr, fg_, fb) = (
                (fg[i] >> 24) & 0xFF, (fg[i] >> 16) & 0xFF, (fg[i] >> 8) & 0xFF, fg[i] & 0xFF,
            );
            let (ba, br, bg_, bb) = (
                (bg[i] >> 24) & 0xFF, (bg[i] >> 16) & 0xFF, (bg[i] >> 8) & 0xFF, bg[i] & 0xFF,
            );
            let inv = 255 - fa;
            let oa = fa + ba * inv / 255;
            let (or, og, ob) = if oa == 0 {
                (0, 0, 0)
            } else {
                (
                    (fr * fa + br * ba * inv / 255) / oa,
                    (fg_ * fa + bg_ * ba * inv / 255) / oa,
                    (fb * fa + bb * ba * inv / 255) / oa,
                )
            };
            out[i] = (oa << 24) | (or << 16) | (og << 8) | ob;
        }
        out
    }

    /// Create a 32bpp per-pixel-alpha `HICON` from straight ARGB pixels.
    /// Returns a null handle on failure.
    fn create_alpha_icon(argb: &[u32], w: i32, h: i32) -> HICON {
        if w <= 0 || h <= 0 || (argb.len() as i32) < w * h {
            return ptr::null_mut();
        }
        let bmi = BITMAPINFO {
            bmi_header: BITMAPINFOHEADER {
                bi_size: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                bi_width: w, bi_height: -h, // negative = top-down
                bi_planes: 1, bi_bit_count: 32, bi_compression: BI_RGB,
                bi_size_image: 0, bi_x_pels_per_meter: 0, bi_y_pels_per_meter: 0,
                bi_clr_used: 0, bi_clr_important: 0,
            },
            bmi_colors: [0],
        };

        let hdc = unsafe { GetDC(ptr::null_mut()) };
        let mut bits: *mut u8 = ptr::null_mut();
        let hbm_color = unsafe {
            CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits, ptr::null_mut(), 0)
        };
        if hbm_color == 0 || bits.is_null() {
            unsafe { ReleaseDC(ptr::null_mut(), hdc) };
            return ptr::null_mut();
        }

        // Fill the color bitmap with BGRA (straight alpha).
        let dst = unsafe { std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize) };
        for i in 0..(w * h) as usize {
            let a = (argb[i] >> 24) & 0xFF;
            let r = (argb[i] >> 16) & 0xFF;
            let g = (argb[i] >> 8) & 0xFF;
            let b = argb[i] & 0xFF;
            dst[i] = b | (g << 8) | (r << 16) | (a << 24);
        }

        // Monochrome AND mask; all-zero means "use the color bitmap's alpha".
        let hbm_mask = unsafe { CreateBitmap(w, h, 1, 1, ptr::null()) };
        let info = ICONINFO {
            f_icon: 1, x_hotspot: 0, y_hotspot: 0, hbm_mask, hbm_color,
        };
        let hicon = unsafe { CreateIconIndirect(&info) };

        unsafe {
            DeleteObject(hbm_color);
            DeleteObject(hbm_mask);
            ReleaseDC(ptr::null_mut(), hdc);
        }
        hicon
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

        // 2. Title-bar buttons (minimize + close) — alpha-blend over background.
        let state_index = |st: BtnState| match st {
            BtnState::Normal  => 0,
            BtnState::Hover   => 1,
            BtnState::Pressed => 2,
        };
        let mut draw_button = |pixels: &[u32], ox: i32, oy: i32| {
            for by in 0..BTN_H {
                for bx in 0..BTN_W {
                    let dx = ox + bx;
                    let dy = oy + by;
                    if dx < 0 || dx >= WIN_W || dy < 0 || dy >= WIN_H { continue; }
                    let si = (by * BTN_W + bx) as usize;
                    if si >= pixels.len() { continue; }
                    let di = (dy * WIN_W + dx) as usize;
                    dib[di] = alpha_blend_dib(dib[di], to_dib(pixels[si]));
                }
            }
        };
        draw_button(&s.min_btn_pixels[state_index(s.min_btn_state)], MIN_BTN_X, MIN_BTN_Y);
        draw_button(&s.btn_pixels[state_index(s.btn_state)], BTN_X, BTN_Y);

        // 3. Progress bar — two-layer track + fill, blended over background.
        //
        //   Track (drawn first, always full length PROG_TOTAL_W):
        //     A (left cap) + F tiled to fill the middle + E (right cap).
        //   Fill (drawn on top, grows from 0 to PROG_TOTAL_W - A_W - E_W):
        //     B (start cap) + C tiled + D (end cap), positioned right after A.
        //
        // At fill_len == 0 only the track (A, F..., E) is visible. As
        // fill_len grows, B/C/D are painted over the track, progressively
        // covering more of F, until the fill reaches up to E.
        let blit_slice = |dib: &mut [u32], pixels: &[u32], slice_w: i32,
                           dst_x: i32, src_x_off: i32, draw_w: i32| {
            if draw_w <= 0 || slice_w <= 0 || pixels.is_empty() { return; }
            for py in 0..PROG_H {
                for col in 0..draw_w {
                    let src_x = src_x_off + col;
                    if src_x < 0 || src_x >= slice_w { continue; }
                    let dx = dst_x + col;
                    let dy = PROG_Y + py;
                    if dx < 0 || dx >= WIN_W || dy < 0 || dy >= WIN_H { continue; }
                    let si = (py * slice_w + src_x) as usize;
                    if si >= pixels.len() { continue; }
                    let di = (dy * WIN_W + dx) as usize;
                    dib[di] = alpha_blend_dib(dib[di], to_dib(pixels[si]));
                }
            }
        };

        let (a_px, a_w) = &s.track_a;
        let (e_px, e_w) = &s.track_e;
        let (f_px, f_w) = &s.track_f;
        let (b_px, b_w) = &s.fill_b;
        let (c_px, c_w) = &s.fill_c;
        let (d_px, d_w) = &s.fill_d;

        // ── Track (bottom layer): A, then E tiled/clipped to fill the
        // remaining width, then F ───────────────────────────────────────────
        blit_slice(dib, a_px, *a_w, PROG_X, 0, *a_w);
        let e_area_x = PROG_X + a_w;
        let e_area_w = (PROG_TOTAL_W - a_w - f_w).max(0);
        {
            let mut drawn = 0;
            while drawn < e_area_w {
                let w = (*e_w).min(e_area_w - drawn);
                blit_slice(dib, e_px, *e_w, e_area_x + drawn, 0, w);
                drawn += w;
            }
        }
        blit_slice(dib, f_px, *f_w, PROG_X + PROG_TOTAL_W - f_w, 0, *f_w);

        // ── Fill (top layer): B + tiled C + D, grown to reflect the model's
        // progress ratio. Starts at the same x as A and grows to cover the
        // entire track (A + E + F, i.e. all of PROG_TOTAL_W) ────────────────
        let (label1_text, label2_text, label3_text, progress_ratio, _close) = s.snapshot();
        let fill_len = (PROG_TOTAL_W as f64 * progress_ratio as f64).round() as i32;

        if fill_len > 0 {
            let fill_x0 = PROG_X;

            if fill_len <= *b_w {
                blit_slice(dib, b_px, *b_w, fill_x0, 0, fill_len);
            } else {
                blit_slice(dib, b_px, *b_w, fill_x0, 0, *b_w);
                let mut cursor = fill_x0 + b_w;

                let tail_w = *d_w;
                let mid_end = (fill_len - tail_w).max(*b_w);
                let mid_w = (mid_end - b_w).max(0);
                let mut drawn = 0;
                while drawn < mid_w {
                    let w = (*c_w).min(mid_w - drawn);
                    blit_slice(dib, c_px, *c_w, cursor, 0, w);
                    cursor += w;
                    drawn += w;
                }

                let d_visible = (fill_len - b_w - mid_w).clamp(0, *d_w);
                blit_slice(dib, d_px, *d_w, cursor, 0, d_visible);
            }
        }

        // 4. Status label.
        //
        // TextOutW only ever writes RGB into a 32-bit DIB — it never touches
        // the alpha channel. Drawing straight onto `hdc_mem` would leave each
        // glyph pixel with whatever alpha the background already had there,
        // which is why the text came out looking faded/transparent instead
        // of solid.
        //
        // Instead we render the glyphs (white on black) into a separate mask
        // DC, use each pixel's brightness as anti-aliased alpha coverage,
        // and alpha-blend the solid label color through our existing
        // premultiplied blend function — same compositing path as every
        // other layer.
        let draw_label = |dib: &mut [u32], text: &str, x: i32, y: i32, color: (u8, u8, u8)| {
            let wide_text: Vec<u16> = text.encode_utf16().collect();
            let Some((mask, mask_w, mask_h)) = render_text_mask(hdc_screen, s.label_font, &wide_text)
            else { return };
            let (cr, cg, cb) = color;
            for my in 0..mask_h {
                for mx in 0..mask_w {
                    let coverage = mask[(my * mask_w + mx) as usize];
                    if coverage == 0 { continue; }
                    let dx = x + mx;
                    let dy = y + my;
                    if dx < 0 || dx >= WIN_W || dy < 0 || dy >= WIN_H { continue; }
                    let a = coverage as u32;
                    let r = (cr as u32) * a / 255;
                    let g = (cg as u32) * a / 255;
                    let b = (cb as u32) * a / 255;
                    let src_dib = b | (g << 8) | (r << 16) | (a << 24);
                    let di = (dy * WIN_W + dx) as usize;
                    dib[di] = alpha_blend_dib(dib[di], src_dib);
                }
            }
        };

        draw_label(dib, &label2_text, LABEL2_X, LABEL2_Y,
            (LABEL2_COLOR_R, LABEL2_COLOR_G, LABEL2_COLOR_B));
        draw_label(dib, &label1_text, LABEL_X, LABEL_Y,
            (LABEL_COLOR_R, LABEL_COLOR_G, LABEL_COLOR_B));
        // cmsdl's own version, static, same style as label1.
        draw_label(dib, LABEL_VER_TEXT, LABEL_VER_X, LABEL_VER_Y,
            (LABEL_COLOR_R, LABEL_COLOR_G, LABEL_COLOR_B));
        // ETA label, right-aligned to the progress bar, same style as label2.
        if !label3_text.is_empty() {
            let text_w = {
                let wide: Vec<u16> = label3_text.encode_utf16().collect();
                if wide.is_empty() { 0 }
                else {
                    let hdc_tmp = unsafe { CreateCompatibleDC(hdc_screen) };
                    unsafe { SelectObject(hdc_tmp, s.label_font) };
                    let mut extent = SIZE { cx: 0, cy: 0 };
                    unsafe { GetTextExtentPoint32W(hdc_tmp, wide.as_ptr(), wide.len() as i32, &mut extent) };
                    unsafe { DeleteDC(hdc_tmp) };
                    extent.cx
                }
            };
            let label3_x = (LABEL3_RIGHT_X - text_w).max(0);
            draw_label(dib, &label3_text, label3_x, LABEL3_Y,
                (LABEL3_COLOR_R, LABEL3_COLOR_G, LABEL3_COLOR_B));
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

    /// Render `text` with `font` into an off-screen white-on-black DIB, then
    /// convert it to an 8-bit alpha coverage mask (0 = fully transparent,
    /// 255 = fully covered). This lets us treat anti-aliased glyph edges as
    /// real alpha for compositing into our layered window's ARGB surface,
    /// which plain TextOutW-onto-ARGB-DIB cannot do (it never writes alpha).
    ///
    /// Returns `(mask, width, height)` in pixels, or `None` on failure.
    fn render_text_mask(hdc_screen: HDC, font: HGDIOBJ, text: &[u16]) -> Option<(Vec<u8>, i32, i32)> {
        if text.is_empty() { return None; }

        let hdc_mask = unsafe { CreateCompatibleDC(hdc_screen) };
        if hdc_mask.is_null() { return None; }
        unsafe { SelectObject(hdc_mask, font) };

        // Measure the text to size the mask bitmap.
        let mut extent = SIZE { cx: 0, cy: 0 };
        unsafe { GetTextExtentPoint32W(hdc_mask, text.as_ptr(), text.len() as i32, &mut extent) };
        let w = extent.cx.max(1);
        let h = extent.cy.max(1);

        let bmi = BITMAPINFO {
            bmi_header: BITMAPINFOHEADER {
                bi_size: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                bi_width: w, bi_height: -h,
                bi_planes: 1, bi_bit_count: 32, bi_compression: BI_RGB,
                bi_size_image: 0, bi_x_pels_per_meter: 0, bi_y_pels_per_meter: 0,
                bi_clr_used: 0, bi_clr_important: 0,
            },
            bmi_colors: [0],
        };
        let mut bits: *mut u8 = ptr::null_mut();
        let hbmp = unsafe {
            CreateDIBSection(hdc_screen, &bmi, DIB_RGB_COLORS, &mut bits, ptr::null_mut(), 0)
        };
        if hbmp == 0 || bits.is_null() {
            unsafe { DeleteDC(hdc_mask) };
            return None;
        }
        unsafe { SelectObject(hdc_mask, hbmp) };

        // Black background, white text — grayscale value directly becomes
        // our alpha coverage value.
        unsafe {
            SetBkMode(hdc_mask, TRANSPARENT_BKMODE); // no separate bg fill needed
            SetBkColor(hdc_mask, 0x00000000);
            SetTextColor(hdc_mask, 0x00FFFFFF); // white
            TextOutW(hdc_mask, 0, 0, text.as_ptr(), text.len() as i32);
        }

        let pixel_count = (w * h) as usize;
        let px = unsafe { std::slice::from_raw_parts(bits as *const u32, pixel_count) };
        let mut mask = vec![0u8; pixel_count];
        for i in 0..pixel_count {
            // DIB is initialised to zero (black/transparent) by
            // CreateDIBSection; any drawn glyph pixel has R=G=B raised
            // toward white. Use the green channel as the coverage value.
            mask[i] = ((px[i] >> 8) & 0xFF) as u8;
        }

        unsafe {
            DeleteObject(hbmp);
            DeleteDC(hdc_mask);
        }

        Some((mask, w, h))
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
                // Preserve a Pressed state while the button is held (even if the
                // cursor drifts off); otherwise reflect hover.
                let hover_state = |pressed: bool, over: bool| {
                    if pressed { BtnState::Pressed }
                    else if over { BtnState::Hover }
                    else { BtnState::Normal }
                };
                let new_close = hover_state(s.btn_state == BtnState::Pressed, s.hit_close(x, y));
                let new_min = hover_state(s.min_btn_state == BtnState::Pressed, s.hit_min(x, y));
                if new_close != s.btn_state || new_min != s.min_btn_state {
                    s.btn_state = new_close;
                    s.min_btn_state = new_min;
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
                    if s.btn_state != BtnState::Normal || s.min_btn_state != BtnState::Normal {
                        s.btn_state = BtnState::Normal;
                        s.min_btn_state = BtnState::Normal;
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
                } else if s.hit_min(x, y) {
                    s.min_btn_state = BtnState::Pressed;
                    update_layered(s, None);
                }
                0
            }

            WM_LBUTTONUP => {
                if sp.is_null() { return 0; }
                let s = unsafe { &mut *sp };
                let x = (l & 0xFFFF) as i16 as i32;
                let y = ((l >> 16) & 0xFFFF) as i16 as i32;
                let close_was_pressed = s.btn_state == BtnState::Pressed;
                let min_was_pressed = s.min_btn_state == BtnState::Pressed;
                s.btn_state = if s.hit_close(x, y) { BtnState::Hover } else { BtnState::Normal };
                s.min_btn_state = if s.hit_min(x, y) { BtnState::Hover } else { BtnState::Normal };
                if close_was_pressed && s.hit_close(x, y) {
                    unsafe { PostQuitMessage(0) };
                } else if min_was_pressed && s.hit_min(x, y) {
                    unsafe { ShowWindow(hwnd, SW_MINIMIZE) };
                    update_layered(s, None);
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
                    let in_close = cx >= BTN_X && cx < BTN_X + BTN_W && cy >= BTN_Y && cy < BTN_Y + BTN_H;
                    let in_min = cx >= MIN_BTN_X && cx < MIN_BTN_X + BTN_W && cy >= MIN_BTN_Y && cy < MIN_BTN_Y + BTN_H;
                    if in_close || in_min {
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

            WM_TIMER => {
                if w == IDT_ANIM && !sp.is_null() {
                    // Repaint to reflect the latest shared UI model, and close
                    // the window if the owning task requested it.
                    let s = unsafe { &mut *sp };
                    let (_, _, _, progress, should_close) = s.snapshot();
                    update_layered(s, None);
                    set_taskbar_progress(s, progress);
                    s.update_taskbar_icon(progress);
                    if should_close {
                        unsafe { KillTimer(hwnd, IDT_ANIM) };
                        unsafe { PostQuitMessage(0) };
                    }
                }
                0
            }

            WM_DESTROY => {
                unsafe { KillTimer(hwnd, IDT_ANIM) };
                if !sp.is_null() {
                    // Clear the taskbar progress indicator before releasing state.
                    {
                        let s = unsafe { &*sp };
                        if let Some(tb) = &s.taskbar {
                            use windows::Win32::Foundation::HWND;
                            use windows::Win32::UI::Shell::TBPF_NOPROGRESS;
                            // SAFETY: valid handle; failure is inconsequential here.
                            unsafe { let _ = tb.SetProgressState(HWND(s.hwnd), TBPF_NOPROGRESS); }
                        }
                    }
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

    /// Mark the process DPI-aware so Windows performs no bitmap scaling of our
    /// layered window. Combined with the fixed-96-DPI font, the dialog renders
    /// pixel-for-pixel identically at 100%, 125%, 150%, etc.
    ///
    /// Prefers Per-Monitor-v2 (Win10 1703+), resolved dynamically so the binary
    /// still loads on older Windows; falls back to the legacy system-aware call.
    fn set_dpi_aware() {
        // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 == (HANDLE)-4
        const PER_MONITOR_AWARE_V2: isize = -4;
        unsafe {
            let user32 = GetModuleHandleW(wide("user32.dll").as_ptr());
            if !user32.is_null() {
                let name = b"SetProcessDpiAwarenessContext\0";
                let proc = GetProcAddress(user32, name.as_ptr());
                if !proc.is_null() {
                    let set_ctx: extern "system" fn(isize) -> BOOL =
                        std::mem::transmute(proc);
                    if set_ctx(PER_MONITOR_AWARE_V2) != 0 {
                        return;
                    }
                }
            }
            // Fallback: legacy system-DPI awareness (available since Vista).
            SetProcessDPIAware();
        }
    }

    pub fn run_window(ui: super::Arc<super::Mutex<super::UiModel>>) -> anyhow::Result<()> {
        // Do this before any window/DC is created so no scaling is applied.
        set_dpi_aware();

        let hinstance = unsafe { GetModuleHandleW(ptr::null()) };
        let class_name = wide("cmsdl_gui");

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
                wide("cmsdl").as_ptr(),
                // WS_SYSMENU | WS_MINIMIZEBOX enable proper minimize/restore
                // machinery (and taskbar animation) even though no caption is
                // drawn — the window is fully custom-painted.
                WS_POPUP | WS_SYSMENU | WS_MINIMIZEBOX,
                win_x, win_y, WIN_W, WIN_H,
                ptr::null_mut(), ptr::null_mut(), hinstance, ptr::null_mut(),
            )
        };
        if hwnd.is_null() { anyhow::bail!("CreateWindowExW failed"); }

        // Load pixel data for all resources.
        let state = Box::new(WindowState {
            hwnd,
            bg_pixels:  load_pixels(BG_PNG, "png").0,
            btn_pixels: [
                load_pixels(BMP_NORMAL, "bmp").0,
                load_pixels(BMP_HOVER,  "bmp").0,
                load_pixels(BMP_CLICK,  "bmp").0,
            ],
            min_btn_pixels: [
                load_pixels(BMP_MIN_NORMAL, "bmp").0,
                load_pixels(BMP_MIN_HOVER,  "bmp").0,
                load_pixels(BMP_MIN_CLICK,  "bmp").0,
            ],
            track_a: load_pixels(BMP_PROG_A, "bmp"),
            track_f: load_pixels(BMP_PROG_F, "bmp"),
            track_e: load_pixels(BMP_PROG_E, "bmp"),
            fill_b:  load_pixels(BMP_PROG_B, "bmp"),
            fill_c:  load_pixels(BMP_PROG_C, "bmp"),
            fill_d:  load_pixels(BMP_PROG_D, "bmp"),
            btn_state:  BtnState::Normal,
            min_btn_state: BtnState::Normal,
            label_font: create_label_font(),
            ui,
            icon_bg:    {
                let (px, w, h) = load_argb(PROGRESS_BG_ICON, "png");
                (px, w, h)
            },
            icon_pct:   -1,
            icon:       ptr::null_mut(),
            taskbar:    init_taskbar(),
        });

        // Attach state to window.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize) };

        // Initial paint with explicit screen position, then show.
        let sp = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
        if !sp.is_null() {
            update_layered(unsafe { &*sp }, Some(POINT { x: win_x, y: win_y }));
        }
        unsafe { ShowWindow(hwnd, 5 /* SW_SHOW */) };

        // Drive the demo animation with a repaint timer.
        unsafe { SetTimer(hwnd, IDT_ANIM, ANIM_TICK_MS, ptr::null()) };

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
