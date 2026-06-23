//! Build script.
//!
//! When building for a Windows target, embed `src/icon.ico` into the resulting
//! executable so it shows the application icon in Explorer and the taskbar.
//! On all other targets this is a no-op.

fn main() {
    // `CARGO_CFG_TARGET_OS` reflects the *target* being built, which is what we
    // want so cross-compiles to Windows still embed the icon.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Re-run only when the icon changes.
        println!("cargo:rerun-if-changed=src/icon.ico");

        let mut res = winresource::WindowsResource::new();
        res.set_icon("src/icon.ico");

        if let Err(e) = res.compile() {
            // Don't hard-fail the build if the resource compiler is unavailable;
            // just warn so the binary is still produced (without the icon).
            println!("cargo:warning=failed to embed Windows icon: {e}");
        }
    }
}
