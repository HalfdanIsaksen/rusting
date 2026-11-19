//! Project files the operating system asks the app to open.
//!
//! On macOS, double-clicking a `.rusting` file in Finder (or dropping it on
//! the Dock icon) does not pass its path on the command line: AppKit calls
//! `application:openURLs:` on the application delegate instead. winit's
//! delegate has no such method and winit has no event for it, so
//! [`install`] adds the method to winit's delegate class at runtime. It
//! queues the paths and wakes the event loop, and the app picks them up with
//! [`take`] in its `user_event` handler.
//!
//! Elsewhere the path arrives on the command line, so this does nothing.

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use winit::event_loop::EventLoopProxy;

/// Paths received but not yet opened, oldest first.
static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
/// Wakes the event loop when paths arrive.
static PROXY: Mutex<Option<EventLoopProxy<()>>> = Mutex::new(None);

/// The paths received since the last call, oldest first.
pub fn take() -> Vec<PathBuf> {
    std::mem::take(&mut *PENDING.lock().unwrap_or_else(PoisonError::into_inner))
}

/// Queues `paths` and wakes the event loop to open them.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn receive(paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    tracing::info!(?paths, "asked to open");
    PENDING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .extend(paths);
    if let Some(proxy) = &*PROXY.lock().unwrap_or_else(PoisonError::into_inner) {
        // Fails only once the loop has exited, when there is nothing to open.
        let _ = proxy.send_event(());
    }
}

/// Starts listening for files to open. Call on the main thread after the
/// event loop is created (it installs winit's delegate) and before it runs
/// (AppKit sends the files a launch was for while the loop starts).
#[cfg(target_os = "macos")]
pub fn install(proxy: EventLoopProxy<()>) {
    use objc2::MainThreadMarker;
    use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
    use objc2::{ffi, sel};
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{NSArray, NSURL};

    /// `-[NSApplicationDelegate application:openURLs:]`.
    ///
    /// SAFETY (for callers): AppKit calls it on the main thread with a valid
    /// array of URLs, or nil.
    #[allow(unsafe_code)]
    unsafe extern "C-unwind" fn open_urls(
        _this: *mut AnyObject,
        _cmd: Sel,
        _app: *mut AnyObject,
        urls: *const NSArray<NSURL>,
    ) {
        // SAFETY: see above; nil becomes `None`.
        let Some(urls) = (unsafe { urls.as_ref() }) else {
            return;
        };
        receive(urls.iter().filter_map(|url| url.to_file_path()).collect());
    }

    *PROXY.lock().unwrap_or_else(PoisonError::into_inner) = Some(proxy);

    let Some(mtm) = MainThreadMarker::new() else {
        tracing::warn!("open files: not on the main thread");
        return;
    };
    let Some(delegate) = NSApplication::sharedApplication(mtm).delegate() else {
        tracing::warn!("open files: the application has no delegate");
        return;
    };
    let object: &AnyObject = delegate.as_ref();
    let class: &AnyClass = object.class();
    let selector = sel!(application:openURLs:);
    if class.responds_to(selector) {
        tracing::warn!(class = %class.name().to_string_lossy(), "open files: delegate already handles files");
        return;
    }
    type OpenUrls =
        unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *const NSArray<NSURL>);
    // SAFETY: `open_urls` has the signature the type encoding below describes
    // (void return; self, _cmd and two objects), and the class is a real,
    // registered class we only add a method to. Adding a method to an
    // existing class is allowed at any time on the main thread.
    #[allow(unsafe_code)]
    let added = unsafe {
        let imp = std::mem::transmute::<OpenUrls, Imp>(open_urls);
        ffi::class_addMethod(
            std::ptr::from_ref(class).cast_mut(),
            selector,
            imp,
            c"v@:@@".as_ptr(),
        )
    };
    if added.as_bool() {
        tracing::debug!("open files: listening");
    } else {
        tracing::warn!("open files: could not add the delegate method");
    }
}

/// Files to open arrive on the command line outside macOS.
#[cfg(not(target_os = "macos"))]
pub fn install(_proxy: EventLoopProxy<()>) {}
