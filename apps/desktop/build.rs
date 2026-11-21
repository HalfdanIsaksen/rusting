//! Embeds the app icon and version details in `rusting.exe` on Windows, so
//! Explorer, Start menu shortcuts and the `.rusting` file type show the logo.
//! Does nothing when building for other platforms.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/Rusting.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    windows_resources();
}

#[cfg(windows)]
fn windows_resources() {
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/Rusting.ico")
        .set("ProductName", "Rusting")
        .set("FileDescription", "Rusting");
    if let Err(err) = res.compile() {
        // A missing resource compiler should not stop the build; the window
        // still sets its icon at runtime.
        println!("cargo:warning=could not embed the Windows icon: {err}");
    }
}

/// Cross-compiling for Windows from another OS: the resource compiler is
/// not available here, so the exe is built without an embedded icon.
#[cfg(not(windows))]
fn windows_resources() {
    println!("cargo:warning=building for Windows from another OS: no embedded icon");
}
