//! HDD / mechanical-disk detection.
//!
//! On Windows, the volume underlying a given path is queried via
//! [`DeviceIoControl`] with `IOCTL_STORAGE_QUERY_PROPERTY` and
//! `StorageDeviceSeekPenaltyProperty`.  A **seek penalty** (i.e. the drive
//! reports `IncursSeekPenalty == 1`) indicates a rotating-platter hard disk;
//! SSDs and other flash-based storage report `0`.
//!
//! The query may fail on certain volume types (network drives, RAM disks,
//! synthetic volumes, etc.).  In that case the function conservatively
//! returns `false` (not an HDD) to avoid false-positive warnings.
//!
//! On non-Windows targets the function always returns `false`.

use std::path::Path;

/// Returns `true` when the storage device backing `path` is a mechanical
/// hard disk (HDD) rather than an SSD or other flash-based media.
///
/// On Windows this uses the seek-penalty IOCTL; any OS or I/O error
/// silently returns `false`.  On non-Windows targets this always returns
/// `false`.
pub fn is_hdd(path: &Path) -> bool {
    #[cfg(windows)]
    {
        imp::check_seek_penalty(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::mem;

    // ── Raw FFI: kernel32.dll ─────────────────────────────────────────────

    extern "system" {
        fn CreateFileW(
            lpFileName: *const u16,
            dwDesiredAccess: u32,
            dwShareMode: u32,
            lpSecurityAttributes: *mut std::ffi::c_void,
            dwCreationDisposition: u32,
            dwFlagsAndAttributes: u32,
            hTemplateFile: *mut std::ffi::c_void,
        ) -> isize;

        fn CloseHandle(hObject: isize) -> i32;

        fn DeviceIoControl(
            hDevice: isize,
            dwIoControlCode: u32,
            lpInBuffer: *const std::ffi::c_void,
            nInBufferSize: u32,
            lpOutBuffer: *mut std::ffi::c_void,
            nOutBufferSize: u32,
            lpBytesReturned: *mut u32,
            lpOverlapped: *mut std::ffi::c_void,
        ) -> i32;
    }

    // ── Constants ─────────────────────────────────────────────────────────

    const INVALID_HANDLE_VALUE: isize = -1;
    const FILE_SHARE_READ: u32 = 1;
    const FILE_SHARE_WRITE: u32 = 2;
    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    const OPEN_EXISTING: u32 = 3;

    // IOCTL codes and property IDs from winioctl.h / ntddstor.h
    const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x2D1400;
    const STORAGE_PROPERTY_QUERY: u32 = 0;
    const STORAGE_SEEK_PENALTY_PROPERTY: u32 = 7; // StorageDeviceSeekPenaltyProperty
    const STORAGE_TRIM_PROPERTY: u32 = 8; // StorageDeviceTrimProperty

    #[repr(C)]
    struct StoragePropertyQuery {
        property_id: u32,   // STORAGE_PROPERTY_ID
        query_type: u32,    // STORAGE_QUERY_TYPE
        additional_parameters: [u8; 4],
    }

    /// DEVICE_SEEK_PENALTY_DESCRIPTOR — version must be set to sizeof(Self)
    /// before the IOCTL call.
    #[repr(C)]
    struct DeviceSeekPenaltyDescriptor {
        version: u32,
        size: u32,
        incurs_seek_penalty: u8, // BOOLEAN
        _reserved: [u8; 3],      // alignment padding
    }

    /// DEVICE_TRIM_DESCRIPTOR — version must be set to sizeof(Self) before
    /// the IOCTL call.
    #[repr(C)]
    struct DeviceTrimDescriptor {
        version: u32,
        size: u32,
        trim_enabled: u8, // BOOLEAN
        _reserved: [u8; 3],
    }

    /// Open the volume device for the root of `path` (e.g. `\\.\D:`) and
    /// query whether the disk incurs a seek penalty.
    pub fn check_seek_penalty(path: &Path) -> bool {
        // Get the volume root (e.g. "D:\").
        let volume_root = match get_volume_root(path) {
            Some(v) => v,
            None => return false,
        };

        // Build the device path "\\.\D:" (no trailing backslash).
        let device_path = format!("\\\\.\\{}", &volume_root[..volume_root.len().saturating_sub(1)]);

        let wide: Vec<u16> = OsStr::new(&device_path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        // Open the physical device (FILE_READ_ATTRIBUTES is required by
        // IOCTL_STORAGE_QUERY_PROPERTY per MSDN).
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_READ_ATTRIBUTES, // dwDesiredAccess
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null_mut(), // lpSecurityAttributes
                OPEN_EXISTING,
                0, // dwFlagsAndAttributes
                std::ptr::null_mut(), // hTemplateFile
            )
        };

        if handle == INVALID_HANDLE_VALUE {
            return false;
        }

        let result = query_seek_penalty(handle);
        // Close handle; ignore failure.
        unsafe { CloseHandle(handle); }
        result
    }

    /// Query the seek-penalty property.  If the device reports that it
    /// incurs a seek penalty, it's an HDD.  If the query fails or the
    /// device reports no seek penalty, we cannot conclude either way
    /// (some USB bridges don't pass this through), so we fall back to
    /// the TRIM property.
    fn query_seek_penalty(handle: isize) -> bool {
        let mut descriptor = DeviceSeekPenaltyDescriptor {
            version: mem::size_of::<DeviceSeekPenaltyDescriptor>() as u32,
            size: 0,
            incurs_seek_penalty: 0,
            _reserved: [0; 3],
        };

        let query = StoragePropertyQuery {
            property_id: STORAGE_SEEK_PENALTY_PROPERTY,
            query_type: STORAGE_PROPERTY_QUERY,
            additional_parameters: [0u8; 4],
        };

        let mut bytes_returned: u32 = 0;
        let ret = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_STORAGE_QUERY_PROPERTY,
                &query as *const _ as *const std::ffi::c_void,
                mem::size_of::<StoragePropertyQuery>() as u32,
                &mut descriptor as *mut _ as *mut std::ffi::c_void,
                mem::size_of::<DeviceSeekPenaltyDescriptor>() as u32,
                &mut bytes_returned,
                std::ptr::null_mut(),
            )
        };

        if ret != 0 && bytes_returned >= mem::size_of::<DeviceSeekPenaltyDescriptor>() as u32 {
            if descriptor.incurs_seek_penalty != 0 {
                return true; // definitely HDD
            }
            // Seek penalty is 0 — could be SSD, or could be an HDD behind a
            // USB bridge that doesn't forward the property.  Fall through to
            // the TRIM check.
        }

        // TRIM fallback: SSDs support TRIM; HDDs do not.
        query_trim_support(handle)
    }

    /// Query whether the device supports TRIM.  TRIM is an SSD-only feature.
    fn query_trim_support(handle: isize) -> bool {
        let mut descriptor = DeviceTrimDescriptor {
            version: mem::size_of::<DeviceTrimDescriptor>() as u32,
            size: 0,
            trim_enabled: 0,
            _reserved: [0; 3],
        };

        let query = StoragePropertyQuery {
            property_id: STORAGE_TRIM_PROPERTY,
            query_type: STORAGE_PROPERTY_QUERY,
            additional_parameters: [0u8; 4],
        };

        let mut bytes_returned: u32 = 0;
        let ret = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_STORAGE_QUERY_PROPERTY,
                &query as *const _ as *const std::ffi::c_void,
                mem::size_of::<StoragePropertyQuery>() as u32,
                &mut descriptor as *mut _ as *mut std::ffi::c_void,
                mem::size_of::<DeviceTrimDescriptor>() as u32,
                &mut bytes_returned,
                std::ptr::null_mut(),
            )
        };

        if ret != 0 && bytes_returned >= mem::size_of::<DeviceTrimDescriptor>() as u32 {
            // TRIM enabled → SSD → not HDD
            descriptor.trim_enabled == 0
        } else {
            // Cannot determine.  Conservatively assume not HDD.
            false
        }
    }

    /// Given a path, return its volume root (e.g. `D:\`) or `None` if it
    /// cannot be determined.
    fn get_volume_root(path: &Path) -> Option<String> {
        // Canonicalize if possible to get an absolute path.
        let abs = path.canonicalize().ok()?;
        abs.to_str().and_then(|s| {
            // Handle extended-length paths (\\?\) and UNC paths.
            let rest = if let Some(rest) = s.strip_prefix("\\\\?\\") {
                // Extended-length path: \\?\D:\...  or  \\?\UNC\server\share\...
                if let Some(_unc) = rest.strip_prefix("UNC\\") {
                    // \\?\UNC\server\share → UNC path, skip
                    return None;
                }
                rest
            } else if s.starts_with("\\\\") {
                // Traditional UNC path: \\server\share\...
                return None;
            } else {
                s
            };

            // At this point `rest` should look like "D:\..." or "D:\".
            let bytes = rest.as_bytes();
            if bytes.len() >= 2 && bytes[1] == b':' {
                let drive_letter = bytes[0].to_ascii_uppercase() as char;
                Some(format!("{drive_letter}:\\"))
            } else {
                None
            }
        })
    }
}
