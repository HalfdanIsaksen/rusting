# ADR-026: Check for a newer release at launch

**Status:** Proposed

## Context
People who download the DMG, zip or tarball never hear about new releases unless they visit GitHub or the website. Homebrew users get them only when they run `brew upgrade`. Every release is a GitHub Release tagged `vX.Y.Z` (see `docs/releasing.md`), so GitHub's API already knows the newest version.

## Decision
- **Ask GitHub, tell the user, don't install.** At launch `apps/desktop/src/update_check.rs` asks `GET https://api.github.com/repos/HalfdanIsaksen/rusting/releases/latest` (the newest release that is not a draft or pre-release) and compares its tag with `CARGO_PKG_VERSION`. A newer release opens a modal dialog (`ui::update_dialog`, `crates/ui/src/panels/update.rs`) with the version, the first sentence of its notes and:
  - **Download** (opens the release page), or, when the running program is under a Homebrew folder (`/Cellar/`, `/homebrew/`, `/linuxbrew/`), the `brew upgrade rusting` command with a Copy button and **Release Notes**,
  - **Later** (Escape, or a click outside),
  - **Skip This Version** (no more asking until a newer release),
  - a **Check for updates when Rusting starts** checkbox.
- **Help → Check for Updates…** always asks, and reports in the status bar when the app is up to date or the check failed.
- **At most one request a day.** The answer is kept in `update-check.json` in the configuration folder, next to `panel-layout.json`, with the skipped version and the setting. Within a day the kept answer is used, so **Later** asks again at the next launch without a request.
- **Never in the way.** The request runs on its own thread with a 10 s timeout and wakes the event loop through the existing `EventLoopProxy<()>`. A failure (offline, rate limit, bad answer) is only logged at launch. The dialog waits while another modal (unsaved changes, export) is up.
- HTTP goes through `ureq` 3 with `rustls` and bundled Mozilla roots, so there is no OpenSSL or system TLS dependency. Only `apps/desktop` depends on it. `RUSTING_NO_UPDATE_CHECK=1` turns the check off (CI, development).
- The request sends only a `User-Agent` of `rusting/<version>`. No identifier or usage data.

## Consequences
- The app contacts GitHub once a day at launch unless the user turns it off. The README says so.
- `ureq` adds `rustls`, `ring` and `webpki-roots` to the desktop app's dependencies. `ring` needs a C compiler, which the release builds already have.
- The app never replaces itself. Self-updating would have to re-sign and notarize on macOS and would fight Homebrew. Revisit if the Windows installer (winget) needs it.
- Renaming the GitHub repository or changing the tag format (`vX.Y.Z`) breaks the check for versions already installed.
