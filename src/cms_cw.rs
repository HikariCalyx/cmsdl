//! CMS CW (China mainland region, alternative) specific logic.
//!
//! Only contains functionality that differs from the shared [`crate::cms`]
//! module: shortcut creation and game launching.  All other operations
//! (downloading, patching, file-list discovery, etc.) reuse `crate::cms`.

use anyhow::{bail, Context, Result};
use std::path::Path;

// ── CMS CW API endpoints ────────────────────────────────────────────────────

/// URL of the CMS CW launcher control file (shared with CMS).
pub(crate) const CTRL_XML_URL: &str = "https://downloader.dorado.sdo.com/v3launcher/5/v3ctrl.xml";

/// URL of the CMS CW patch metadata file (`ver2.dat`).
// TODO: update with the actual CMS CW ver2.dat URL.
pub(crate) const PATCH_DATA_URL: &str =
    "https://v3launcher.jijiagames.com/v3launcher/build/ver2data/791001093/8859/-1/ver2.dat";

/// Host serving the signed CMS CW client download files.
pub(crate) const DOWNLOAD_HOST: &str = "https://mxdcclient.jijiagames.com";

/// Portion of the client-file-list path that precedes the build number.
// TODO: update with the actual CMS CW path prefix.
pub(crate) const CLIENT_FILE_LIST_PATH_PREFIX: &str = "/v3client/build/791001093/8859/apppc/";

/// Portion of the client-file-list path that follows the build number.
pub(crate) const CLIENT_FILE_LIST_PATH_SUFFIX: &str = "/client_all_files_list.dat";

/// Build number to start the exhaustive search from.
// TODO: update with the actual CMS CW default starting build number.
pub(crate) const DEFAULT_CLIENT_NUMBER: u32 = 1;

/// Unsigned launcher file list whose header records the current build number.
// TODO: update with the actual CMS CW initial client list URL.
pub(crate) const INITIAL_CLIENT_LIST_URL: &str =
    "https://v3launcher.jijiagames.com/v3launcher/build/791001093/8859/client-all-files-list/client_all_files_list.dat";

// ── Region config ───────────────────────────────────────────────────────────

/// CMS CW configuration, used with [`crate::cms::with_config`] to switch
/// all URL-dependent functions to CW endpoints.
pub(crate) const CW_CONFIG: crate::cms::CmsConfig = crate::cms::CmsConfig {
    ctrl_xml_url: CTRL_XML_URL,
    patch_data_url: PATCH_DATA_URL,
    download_host: DOWNLOAD_HOST,
    client_file_list_path_prefix: CLIENT_FILE_LIST_PATH_PREFIX,
    client_file_list_path_suffix: CLIENT_FILE_LIST_PATH_SUFFIX,
    default_client_number: DEFAULT_CLIENT_NUMBER,
    initial_client_list_url: INITIAL_CLIENT_LIST_URL,
    last_client_version_file: "last_client_version.ini",
    last_client_version_section: "CMS_CW",
    data_dir: "mxdclassic",
    product_id: "791001093",
};

// ── Shared-operation wrappers ───────────────────────────────────────────────

use crate::filter::FileFilter;

/// Download and parse the client file list summary (CW endpoints).
pub fn get_client_file_list_info(allow_insecure: bool, proxy: Option<&str>, build: Option<u32>) -> anyhow::Result<crate::cms::ClientFileList> {
    crate::cms::with_config(CW_CONFIG, || {
        crate::cms::get_client_file_list_info(allow_insecure, proxy, build)
    })
}

/// Download and parse the full client file list (CW endpoints).
pub fn get_client_file_list_full(allow_insecure: bool, proxy: Option<&str>, build: Option<u32>) -> anyhow::Result<(crate::cms::ClientFileList, Vec<(String, u64)>)> {
    crate::cms::with_config(CW_CONFIG, || {
        crate::cms::get_client_file_list_full(allow_insecure, proxy, build)
    })
}

/// List builds from `since` upward (CW endpoints).
pub fn list_builds_since(allow_insecure: bool, proxy: Option<&str>, since: u32) -> anyhow::Result<Vec<crate::cms::BuildInfo>> {
    crate::cms::with_config(CW_CONFIG, || {
        crate::cms::list_builds_since(allow_insecure, proxy, since)
    })
}

/// Download client files for the CW region.
pub fn download_client(target_dir: &Path, wz_only: bool, filter: Option<&FileFilter>, allow_insecure: bool, proxy: Option<&str>, build: Option<u32>, purge_wz_files: bool) -> anyhow::Result<()> {
    crate::cms::with_config(CW_CONFIG, || {
        crate::cms::download_client(target_dir, wz_only, filter, allow_insecure, proxy, build, purge_wz_files)
    })
}

/// Fetch CMS CW patch metadata.
pub fn get_patch_data(allow_insecure: bool, proxy: Option<&str>) -> anyhow::Result<crate::cms::PatchData> {
    crate::cms::with_config(CW_CONFIG, || {
        crate::cms::get_patch_data(allow_insecure, proxy)
    })
}

/// Fetch challenge key from CW control file.
pub fn get_challenge_key(agent: &ureq::Agent) -> anyhow::Result<String> {
    crate::cms::with_config(CW_CONFIG, || {
        crate::cms::get_challenge_key(agent)
    })
}

/// Fetch a CW patch's total size.
pub fn get_patch_total_size(agent: &ureq::Agent, challenge_code: &str, base_url: &str, file_list_url: &str) -> anyhow::Result<u64> {
    crate::cms::with_config(CW_CONFIG, || {
        crate::cms::get_patch_total_size(agent, challenge_code, base_url, file_list_url)
    })
}

// ── Shortcut creation ───────────────────────────────────────────────────────

/// Create a launcher shortcut for the CMS CW client at `target_dir`.
///
/// Uses `cmsdl.exe`'s own icon and targets `cms_cw --patch latest`.
#[cfg(windows)]
pub fn create_shortcut(
    target_dir: &Path,
    lrhook: bool,
    no_gui: bool,
    close_after_finishing: bool,
) -> Result<()> {
    let target_dir = target_dir
        .canonicalize()
        .with_context(|| format!("failed to resolve '{}'", target_dir.display()))?;

    let use_lrhook = lrhook && locale_remulator_available(&target_dir);
    if lrhook && !use_lrhook {
        println!(
            "warning: --lrhook was specified but LocaleRemulator files are missing; \
             shortcut will launch without Locale Remulator."
        );
    }

    let current_exe = std::env::current_exe().context("failed to determine cmsdl binary path")?;
    let cmsdl_in_target = target_dir.join("cmsdl.exe");

    let same_dir = current_exe
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p == target_dir)
        .unwrap_or(false);

    if !same_dir {
        println!("copying cmsdl.exe to '{}'...", target_dir.display());
        std::fs::copy(&current_exe, &cmsdl_in_target)
            .with_context(|| format!("failed to copy cmsdl to '{}'", cmsdl_in_target.display()))?;
    }

    let shortcut_name = if os_locale_is_simplified_chinese() {
        "冒险岛怀旧服"
    } else {
        "MapleStory Classic World CN"
    };
    let lnk_name = format!("{shortcut_name}.lnk");

    // Use cmsdl.exe itself as the icon source.
    run_create_shortcut_script(
        &target_dir,
        &cmsdl_in_target,
        &cmsdl_in_target,
        &lnk_name,
        use_lrhook,
        no_gui,
        close_after_finishing,
        "cms_cw",
    )
}

#[cfg(not(windows))]
pub fn create_shortcut(
    _target_dir: &Path,
    _lrhook: bool,
    _no_gui: bool,
    _close_after_finishing: bool,
) -> Result<()> {
    bail!("--create-shortcut is only supported on Windows")
}

/// Return `true` if all required LocaleRemulator files exist under
/// `<target_dir>/LocaleRemulator/`.
pub fn locale_remulator_available(target_dir: &Path) -> bool {
    let lr = target_dir.join("LocaleRemulator");
    lr.join("LRConfig.xml").is_file()
        && lr.join("LRHookx32.dll").is_file()
        && lr.join("LRHookx64.dll").is_file()
        && lr.join("LRProc.exe").is_file()
        && lr.join("LRSubMenus.dll").is_file()
}

// ── Launch ───────────────────────────────────────────────────────────────────

/// Launch `<target_dir>/mxdclassic/Maplestory_Classic.exe --sqLauncher`.
///
/// The process is spawned without waiting, so cmsdl can exit while the game
/// keeps running.  Locale Remulator (`--lrhook`) is not supported for CMS CW.
#[cfg(windows)]
pub fn launch_client(target_dir: &Path) -> Result<()> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    extern "system" {
        fn ShellExecuteW(
            hwnd: isize,
            lpOperation: *const u16,
            lpFile: *const u16,
            lpParameters: *const u16,
            lpDirectory: *const u16,
            nShowCmd: i32,
        ) -> isize;
    }

    fn to_wide(s: &OsStr) -> Vec<u16> {
        let mut v: Vec<u16> = s.encode_wide().collect();
        v.push(0);
        v
    }

    let mxdc = target_dir.join("mxdclassic");
    let exe = mxdc.join("Maplestory_Classic.exe");
    if !exe.exists() {
        bail!("cannot launch: {} not found", exe.display());
    }

    println!("launching {} --sqLauncher", exe.display());

    let file = to_wide(exe.as_os_str());
    let params = to_wide(OsStr::new("--sqLauncher"));
    let dir = to_wide(mxdc.as_os_str());

    const SW_SHOW: i32 = 5;
    let ret = unsafe {
        ShellExecuteW(0, ptr::null(), file.as_ptr(), params.as_ptr(), dir.as_ptr(), SW_SHOW)
    };

    if ret <= 32 {
        bail!(
            "could not launch the client (ShellExecute error {ret}; \
             the UAC elevation prompt may have been declined)"
        );
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn launch_client(target_dir: &Path) -> Result<()> {
    let mxdc = target_dir.join("mxdclassic");
    let exe = mxdc.join("Maplestory_Classic.exe");
    if !exe.exists() {
        bail!("cannot launch: {} not found", exe.display());
    }
    println!("launching {} --sqLauncher", exe.display());
    std::process::Command::new(&exe)
        .arg("--sqLauncher")
        .current_dir(&mxdc)
        .spawn()
        .with_context(|| format!("failed to launch {}", exe.display()))?;
    Ok(())
}

// ── Internal helpers ─────────────────────────────────────────────────────────

#[cfg(windows)]
fn os_locale_is_simplified_chinese() -> bool {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey(r"Control Panel\International")
        .and_then(|k| k.get_value::<String, _>("LocaleName"))
        .map(|locale: String| locale == "zh-CN" || locale == "zh-SG")
        .unwrap_or(false)
}

#[cfg(windows)]
fn strip_extended_prefix(path: &Path) -> String {
    let s = path.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_owned()
}

#[cfg(windows)]
fn run_create_shortcut_script(
    target_dir: &Path,
    cmsdl_exe: &Path,
    icon_exe: &Path,
    lnk_name: &str,
    include_lrhook: bool,
    no_gui: bool,
    close_after_finishing: bool,
    region_cmd: &str,
) -> Result<()> {
    use windows::{
        core::{Interface, PCWSTR},
        Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        },
        Win32::UI::Shell::IShellLinkW,
    };

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let cmsdl_s = strip_extended_prefix(cmsdl_exe);
    let target_dir_s = strip_extended_prefix(target_dir);
    let icon_s = strip_extended_prefix(icon_exe);
    let explicit_lnk = strip_extended_prefix(&target_dir.join(lnk_name));

    let lrhook_flag = if include_lrhook { " --lrhook" } else { "" };
    let nogui_flag = if no_gui { " --no-gui" } else { "" };
    let close_flag = if close_after_finishing && !no_gui { " --close-after-finishing" } else { "" };
    let args = format!(
        "{region_cmd} --patch latest \"{target_dir_s}\" --launch-after-patching{lrhook_flag}{nogui_flag}{close_flag}"
    );

    let desktop = get_shell_folder("Desktop")
        .ok_or_else(|| anyhow::anyhow!("could not determine Desktop folder path"))?;
    let programs = get_shell_folder("Programs")
        .ok_or_else(|| anyhow::anyhow!("could not determine Programs folder path"))?;

    let lnk_paths = [
        explicit_lnk,
        format!(r"{desktop}\{lnk_name}"),
        format!(r"{programs}\{lnk_name}"),
    ];

    const CLSID_SHELL_LINK: windows::core::GUID = windows::core::GUID {
        data1: 0x0002_1401,
        data2: 0x0000,
        data3: 0x0000,
        data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
    };

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let shell_link: IShellLinkW =
            CoCreateInstance(&CLSID_SHELL_LINK, None, CLSCTX_INPROC_SERVER)
                .context("failed to create IShellLink COM object")?;

        let target_w = to_wide(&cmsdl_s);
        let args_w = to_wide(&args);
        let wd_w = to_wide(&target_dir_s);
        let icon_w = to_wide(&icon_s);

        shell_link
            .SetPath(PCWSTR(target_w.as_ptr()))
            .context("IShellLink::SetPath failed")?;
        shell_link
            .SetArguments(PCWSTR(args_w.as_ptr()))
            .context("IShellLink::SetArguments failed")?;
        shell_link
            .SetWorkingDirectory(PCWSTR(wd_w.as_ptr()))
            .context("IShellLink::SetWorkingDirectory failed")?;
        shell_link
            .SetIconLocation(PCWSTR(icon_w.as_ptr()), 0)
            .context("IShellLink::SetIconLocation failed")?;

        let persist: IPersistFile = shell_link
            .cast()
            .context("failed to obtain IPersistFile interface")?;

        for lnk_path in &lnk_paths {
            if let Some(parent) = Path::new(lnk_path).parent() {
                std::fs::create_dir_all(parent).ok();
            }
            let lnk_w = to_wide(lnk_path);
            persist
                .Save(PCWSTR(lnk_w.as_ptr()), true)
                .with_context(|| format!("failed to save shortcut to '{lnk_path}'"))?;
        }
    }

    println!(
        "created shortcut '{lnk_name}' at the desktop, Start Menu, and '{}'.",
        target_dir.display()
    );
    Ok(())
}

#[cfg(windows)]
fn get_shell_folder(name: &str) -> Option<String> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Shell Folders")
        .and_then(|k| k.get_value::<String, _>(name))
        .ok()
}
