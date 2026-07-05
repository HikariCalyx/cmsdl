//! Android-hotspot metered-connection detection.
//!
//! On Windows, every DHCP-enabled network adapter is queried for DHCP option 43
//! (Vendor-Specific Information). When Android shares a **metered** mobile data
//! connection via USB or Wi-Fi tethering it injects the ASCII string
//! `ANDROID_METERED` into that option, signalling to connected clients that
//! downloading large amounts of data may incur charges.
//!
//! A secondary helper, [`is_android_tethering`], checks whether any adapter
//! address falls in a well-known Android tethering subnet — a necessary but not
//! sufficient condition for metered status:
//!
//! | Transport | Subnet            |
//! |-----------|-------------------|
//! | USB       | `192.168.42.0/24` |
//! | Wi-Fi     | `192.168.43.0/24` |
//!
//! Both functions return `false` on non-Windows targets and on any OS error.

/// Returns `true` when the active connection is a **metered** Android hotspot
/// (USB or Wi-Fi tethering).
///
/// Detection is based on the presence of `ANDROID_METERED` in DHCP option 43
/// on any DHCP-enabled adapter.  Any OS error silently returns `false`.
pub fn is_android_metered() -> bool {
    #[cfg(windows)]
    {
        imp::check_metered()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

// /// Returns `true` when any adapter's unicast IPv4 address falls within a
// /// well-known Android tethering subnet, regardless of metered status.
// ///
// /// Use [`is_android_metered`] to additionally verify that the connection is
// /// data-capped.
// pub fn is_android_tethering() -> bool {
//     #[cfg(windows)]
//     {
//         imp::check_tethering()
//     }
//     #[cfg(not(windows))]
//     {
//         false
//     }
// }

/// Returns `true` when any adapter's unicast IPv4 address falls within
/// Apple's Personal Hotspot subnet (`172.20.10.0/28`).
///
/// This subnet is used by iPhone for **all** tethering transports — USB,
/// Wi-Fi, and Bluetooth.  iOS does not advertise metered status via DHCP, so
/// this is the only available indicator that the upstream connection is a
/// cellular link.
pub fn is_iphone_tethering() -> bool {
    #[cfg(windows)]
    {
        imp::check_iphone_tethering()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Returns `true` when the operating system considers the active internet
/// connection to be metered.
///
/// This is detected via the Windows Network List Manager COM interface
/// (`INetworkCostManager`).  It covers two scenarios:
///
/// 1. The user has manually enabled **"Set as metered connection"** in Windows
///    Settings (Network & Internet → adapter properties).
/// 2. Windows has **automatically** classified the link as metered — e.g.
///    a mobile broadband adapter, a cellular USB dongle, or any hotspot that
///    Windows recognises as such.
///
/// Any COM or OS failure is treated silently as "not metered".
pub fn is_windows_metered() -> bool {
    #[cfg(windows)]
    {
        imp::check_windows_metered()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::CStr;
    use std::ptr;

    use windows::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GET_ADAPTERS_ADDRESSES_FLAGS, IP_ADAPTER_ADDRESSES_LH,
        IP_ADAPTER_UNICAST_ADDRESS_LH,
    };
    use windows::Win32::Networking::WinSock::SOCKADDR;
    use windows::Win32::Networking::NetworkListManager::INetworkCostManager;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
    };
    use windows::core::GUID;

    // ── Constants ─────────────────────────────────────────────────────────────

    /// `family` = AF_UNSPEC (0): enumerate both IPv4 and IPv6 adapters.
    const AF_UNSPEC: u32 = 0;

    const ERROR_BUFFER_OVERFLOW: u32 = 111;

    /// Bit 2 of `IP_ADAPTER_ADDRESSES_LH::Flags`: DHCPv4 is active.
    const IP_ADAPTER_DHCP_ENABLED: u32 = 0x0004;

    /// DHCP option 43 — Vendor-Specific Information.
    const DHCP_OPT_VENDOR_SPECIFIC: u32 = 43;

    /// The payload Android injects into option 43 when mobile data is metered.
    const ANDROID_METERED: &[u8] = b"ANDROID_METERED";

    // /// First three octets identifying the Android USB-tethering subnet
    // /// (`192.168.42.0/24`).
    // const ANDROID_USB_PREFIX: [u8; 3] = [192, 168, 42];

    // /// First three octets identifying the Android Wi-Fi-hotspot subnet
    // /// (`192.168.43.0/24`).
    // const ANDROID_WIFI_PREFIX: [u8; 3] = [192, 168, 43];

    /// Apple Personal Hotspot subnet: `172.20.10.0/28`.
    /// The `/28` mask means only the lower 4 bits of the last octet vary
    /// (host addresses `172.20.10.1`–`172.20.10.14`); we check that the
    /// upper 28 bits match by masking the last octet with `0xF0`.
    const IPHONE_HOTSPOT_PREFIX: [u8; 3] = [172, 20, 10];

    /// CLSID for the `NetworkListManager` COM class.
    /// `{DCB00C01-570F-4A9B-8D69-199FDBA5723B}`
    const CLSID_NLM: GUID = GUID {
        data1: 0xdcb0_0c01,
        data2: 0x570f,
        data3: 0x4a9b,
        data4: [0x8d, 0x69, 0x19, 0x9f, 0xdb, 0xa5, 0x72, 0x3b],
    };

    /// `NLM_CONNECTION_COST_FIXED` (0x2) | `NLM_CONNECTION_COST_VARIABLE` (0x4):
    /// both bits indicate a metered connection as reported by Windows.
    const NLM_METERED_MASK: u32 = 0x0002 | 0x0004;

    // ── Raw FFI: dhcpcsvc.dll ─────────────────────────────────────────────────
    //
    // `DhcpRequestParams` is not exposed by the `windows` crate features already
    // in use, so we bind it by hand from dhcpcsvc.dll.
    //
    // The SDK header declares the `RecdParams` parameter as `DHCPAPI_PARAMS`
    // (by value).  On x64 Windows the ABI passes structs larger than 8 bytes via
    // a hidden pointer, so `*mut DhcpApiParams` is ABI-equivalent and lets the
    // caller observe the filled-in `data` and `n_bytes_data` fields after the
    // call.

    #[repr(C)]
    struct DhcpApiParams {
        /// Reserved; must be zero.
        flags: u32,
        /// The DHCP option ID to retrieve.
        option_id: u32,
        /// `FALSE` (0) for standard options; `TRUE` for vendor-specific.
        is_vendor: i32,
        /// Out: pointer into `Buffer` where the option value was written.
        data: *mut u8,
        /// Out: number of valid bytes at `data`.
        n_bytes_data: u32,
    }

    /// `DHCPREQUEST_PERSISTENT` (0x01) | `DHCPREQUEST_SYNCHRONOUS` (0x02):
    /// block until the DHCP server replies and persist the option request for
    /// future lease renewals.
    const DHCPREQUEST_FLAGS: u32 = 0x01 | 0x02;

    #[link(name = "dhcpcsvc", kind = "raw-dylib")]
    extern "system" {
        fn DhcpRequestParams(
            flags: u32,
            reserved: *mut core::ffi::c_void,
            adapter_name: *const u16,
            send_params: *mut DhcpApiParams,
            recv_params: *mut DhcpApiParams,
            buffer: *mut u8,
            p_size: *mut u32,
            request_id_str: *const u16,
        ) -> u32;
    }

    // ── Public entry points ───────────────────────────────────────────────────

    pub(super) fn check_metered() -> bool {
        walk_adapters(|adapter| {
            if !dhcp_enabled(adapter) {
                return false;
            }
            option_43_is_android_metered(adapter)
        })
    }

    // pub(super) fn check_tethering() -> bool {
    //     walk_adapters(|adapter| {
    //         if !dhcp_enabled(adapter) {
    //             return false;
    //         }
    //         unicast_in_android_subnet(adapter)
    //     })
    // }

    pub(super) fn check_iphone_tethering() -> bool {
        walk_adapters(|adapter| unicast_in_iphone_subnet(adapter))
    }

    pub(super) fn check_windows_metered() -> bool {
        unsafe {
            // S_OK (0)  → we initialised COM; must uninitialise afterward.
            // S_FALSE (1) → already initialised on this thread; the ref-count
            //               was still incremented, so we uninitialise as well.
            // Any negative HRESULT → failure; COM unavailable, return false.
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            if hr.is_err() {
                return false;
            }
            let metered = nlm_cost_is_metered();
            CoUninitialize();
            metered
        }
    }

    /// Instantiates `INetworkCostManager` and queries the internet-connection
    /// cost.  Returns `false` on any COM error.
    fn nlm_cost_is_metered() -> bool {
        unsafe {
            let Ok(mgr) = CoCreateInstance::<_, INetworkCostManager>(
                &CLSID_NLM,
                None,
                CLSCTX_ALL,
            ) else {
                return false;
            };
            let mut cost: u32 = 0;
            if mgr.GetCost(&mut cost, ptr::null()).is_err() {
                return false;
            }
            // FIXED (0x2) = metered with a data cap.
            // VARIABLE (0x4) = metered per-byte (e.g. cellular).
            (cost & NLM_METERED_MASK) != 0
        }
    }

    // ── Adapter enumeration ───────────────────────────────────────────────────

    /// Calls `f` for each adapter; returns `true` as soon as `f` does.
    fn walk_adapters<F>(f: F) -> bool
    where
        F: Fn(&IP_ADAPTER_ADDRESSES_LH) -> bool,
    {
        let buf = match alloc_adapter_buffer() {
            Some(b) => b,
            None => return false,
        };
        let mut cursor: *const IP_ADAPTER_ADDRESSES_LH =
            buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !cursor.is_null() {
            let adapter = unsafe { &*cursor };
            if f(adapter) {
                return true;
            }
            cursor = adapter.Next as *const IP_ADAPTER_ADDRESSES_LH;
        }
        false
    }

    /// Calls `GetAdaptersAddresses` twice: first to learn the required buffer
    /// size, then to actually fill it.  Returns `None` on any error.
    fn alloc_adapter_buffer() -> Option<Vec<u8>> {
        // Skip address types we don't need to reduce allocations.
        let flags = GET_ADAPTERS_ADDRESSES_FLAGS(0x0002 | 0x0004 | 0x0008);
        let mut size: u32 = 0;
        let rc =
            unsafe { GetAdaptersAddresses(AF_UNSPEC, flags, None, None, &mut size) };
        if rc != ERROR_BUFFER_OVERFLOW {
            return None;
        }

        let mut buf = vec![0u8; size as usize];
        let rc = unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC,
                flags,
                None,
                Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH),
                &mut size,
            )
        };
        if rc != 0 {
            return None;
        }
        Some(buf)
    }

    // ── DHCP option 43 check ──────────────────────────────────────────────────

    fn dhcp_enabled(adapter: &IP_ADAPTER_ADDRESSES_LH) -> bool {
        // `Flags` lives inside the `Anonymous2` union of `IP_ADAPTER_ADDRESSES_LH`.
        (unsafe { adapter.Anonymous2.Flags } & IP_ADAPTER_DHCP_ENABLED) != 0
    }

    fn option_43_is_android_metered(adapter: &IP_ADAPTER_ADDRESSES_LH) -> bool {
        // `AdapterName` is a PSTR GUID string such as `{A1B2-...}`.
        // `DhcpRequestParams` expects the device path `\DEVICE\TCPIP_<GUID>`.
        let adapter_name = unsafe {
            if adapter.AdapterName.0.is_null() {
                return false;
            }
            match CStr::from_ptr(adapter.AdapterName.0 as *const i8).to_str() {
                Ok(s) => s.to_owned(),
                Err(_) => return false,
            }
        };

        let device_path = format!("\\DEVICE\\TCPIP_{adapter_name}");
        let wide: Vec<u16> = device_path
            .encode_utf16()
            .chain(std::iter::once(0u16))
            .collect();

        let mut recv = DhcpApiParams {
            flags: 0,
            option_id: DHCP_OPT_VENDOR_SPECIFIC,
            is_vendor: 0,
            data: ptr::null_mut(),
            n_bytes_data: 0,
        };

        // DHCP option values are at most 255 bytes; 256 is always sufficient.
        let mut buf_size: u32 = 256;
        let mut buf = vec![0u8; 256];

        let rc = unsafe {
            DhcpRequestParams(
                DHCPREQUEST_FLAGS,
                ptr::null_mut(),
                wide.as_ptr(),
                ptr::null_mut(),
                &mut recv,
                buf.as_mut_ptr(),
                &mut buf_size,
                ptr::null(),
            )
        };

        if rc != 0 || recv.data.is_null() || recv.n_bytes_data == 0 {
            return false;
        }

        // `recv.data` points into `buf`; it is valid for `n_bytes_data` bytes.
        let data =
            unsafe { std::slice::from_raw_parts(recv.data, recv.n_bytes_data as usize) };
        data.windows(ANDROID_METERED.len())
            .any(|w| w == ANDROID_METERED)
    }

    // ── Subnet heuristic ─────────────────────────────────────────────────────

    fn unicast_in_iphone_subnet(adapter: &IP_ADAPTER_ADDRESSES_LH) -> bool {
        // iPhone does not require DHCP to be enabled — USB tethering uses a
        // static address assignment from the iPhone itself, and the adapter
        // may not show the DHCP flag even when the lease came from iOS.
        let mut ua: *const IP_ADAPTER_UNICAST_ADDRESS_LH =
            adapter.FirstUnicastAddress as *const IP_ADAPTER_UNICAST_ADDRESS_LH;
        while !ua.is_null() {
            let entry = unsafe { &*ua };
            if let Some(ip) = ipv4_from_unicast_addr(entry) {
                let oct = ip.octets();
                // Match 172.20.10.0/28: first 3 octets exact, last octet 0–15.
                if oct[..3] == IPHONE_HOTSPOT_PREFIX && (oct[3] & 0xF0) == 0 {
                    return true;
                }
            }
            ua = entry.Next as *const IP_ADAPTER_UNICAST_ADDRESS_LH;
        }
        false
    }

    // fn unicast_in_android_subnet(adapter: &IP_ADAPTER_ADDRESSES_LH) -> bool {
    //     let mut ua: *const IP_ADAPTER_UNICAST_ADDRESS_LH =
    //         adapter.FirstUnicastAddress as *const IP_ADAPTER_UNICAST_ADDRESS_LH;
    //     while !ua.is_null() {
    //         let entry = unsafe { &*ua };
    //         if let Some(ip) = ipv4_from_unicast_addr(entry) {
    //             let oct = ip.octets();
    //             if oct[..3] == ANDROID_USB_PREFIX || oct[..3] == ANDROID_WIFI_PREFIX {
    //                 return true;
    //             }
    //         }
    //         ua = entry.Next as *const IP_ADAPTER_UNICAST_ADDRESS_LH;
    //     }
    //     false
    // }

    fn ipv4_from_unicast_addr(
        ua: &IP_ADAPTER_UNICAST_ADDRESS_LH,
    ) -> Option<std::net::Ipv4Addr> {
        let sa = &ua.Address;
        if sa.iSockaddrLength < 8 || sa.lpSockaddr.is_null() {
            return None;
        }
        let raw = unsafe { &*(sa.lpSockaddr as *const SOCKADDR) };
        // `ADDRESS_FAMILY(2)` = AF_INET.
        if raw.sa_family.0 != 2 {
            return None;
        }
        // `SOCKADDR` layout: sa_family (u16) | sa_data (14 × i8)
        // Overlaid `SOCKADDR_IN`:  sin_family | sin_port (2 B) | sin_addr (4 B)
        // sin_port occupies sa_data[0..2]; sin_addr occupies sa_data[2..6].
        let d = raw.sa_data;
        Some(std::net::Ipv4Addr::new(
            d[2] as u8,
            d[3] as u8,
            d[4] as u8,
            d[5] as u8,
        ))
    }
}
