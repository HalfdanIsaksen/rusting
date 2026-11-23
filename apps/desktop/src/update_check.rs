//! Looks for a newer release on GitHub when the app starts (ADR-026), and
//! shows [`ui::update_dialog`] when there is one.
//!
//! - At most one request a day. In between, the release found last time is
//!   reused, so "Later" still asks again at the next launch.
//! - The request runs on its own thread and wakes the event loop when done.
//!   Being offline, or any other failure, is only logged.
//! - The dialog's checkbox turns the check off; Help → Check for Updates…
//!   always asks. `RUSTING_NO_UPDATE_CHECK=1` turns it off too (for CI and
//!   development).
//!
//! The state is kept as JSON next to the panel layout
//! (`update-check.json` in [`crate::layout_file::config_dir`]).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// GitHub's API for the newest published (not draft, not pre-release) release.
const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/HalfdanIsaksen/rusting/releases/latest";
/// How long a check's answer is trusted before asking GitHub again.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// How long one request may take before it is given up.
const TIMEOUT: Duration = Duration::from_secs(10);
/// File name of the saved state.
const FILE_NAME: &str = "update-check.json";
/// The longest summary shown, in characters.
const MAX_SUMMARY: usize = 240;

/// A release version: `major.minor.patch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(u64, u64, u64);

impl Version {
    /// Parses `0.2.3` or `v0.2.3`. A pre-release or build suffix
    /// (`-beta.1`, `+abc`) is ignored.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
        let core = text.split(['-', '+']).next()?;
        let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
        let version = Self(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(version)
    }

    /// The version of this build.
    pub fn current() -> Option<Self> {
        Self::parse(env!("CARGO_PKG_VERSION"))
    }
}

/// A published release, as remembered between launches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// The version, without a leading `v`.
    pub version: String,
    /// The release's web page.
    pub url: String,
    /// The first sentence of its notes.
    pub summary: Option<String>,
}

/// The fields of GitHub's release JSON that are used.
#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// Reads GitHub's release JSON. Drafts, pre-releases and tags that are not a
/// version give `None`.
pub fn parse_release(json: &str) -> Option<Release> {
    let release: GithubRelease = serde_json::from_str(json).ok()?;
    if release.draft || release.prerelease {
        return None;
    }
    let version = Version::parse(&release.tag_name)?;
    Some(Release {
        version: format!("{}.{}.{}", version.0, version.1, version.2),
        url: release.html_url,
        summary: release.body.as_deref().and_then(summary),
    })
}

/// The first line of release notes that is a sentence, not a heading, list
/// item or comment. `release-notes.md` starts with one.
fn summary(body: &str) -> Option<String> {
    let line = body.lines().map(str::trim).find(|line| {
        !line.is_empty()
            && !line.starts_with(['#', '-', '*', '<', '|', '!', '>', '`'])
            && !line.starts_with("**")
    })?;
    let mut text: String = line.chars().take(MAX_SUMMARY).collect();
    if text.len() < line.len() {
        text.push('…');
    }
    Some(text)
}

/// Whether the program at `exe` was installed by Homebrew.
pub fn is_homebrew(exe: &Path) -> bool {
    let path = exe.to_string_lossy();
    ["/Cellar/", "/homebrew/", "/linuxbrew/"]
        .iter()
        .any(|part| path.contains(part))
}

/// Whether this running program was installed by Homebrew.
fn running_from_homebrew() -> bool {
    std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .is_ok_and(|exe| is_homebrew(&exe))
}

/// What is kept between launches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct State {
    /// Check at launch.
    enabled: bool,
    /// When GitHub last answered, in seconds since the Unix epoch.
    last_check: Option<u64>,
    /// The newest release GitHub reported then.
    latest: Option<Release>,
    /// A version the user chose to skip.
    skipped: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            enabled: true,
            last_check: None,
            latest: None,
            skipped: None,
        }
    }
}

impl State {
    fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_else(|err| {
                tracing::warn!(path = %path.display(), %err, "ignoring invalid update state");
                Self::default()
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "could not read update state");
                Self::default()
            }
        }
    }

    fn save(&self, path: &Path) {
        let write = || -> std::io::Result<()> {
            let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, json)?;
            std::fs::rename(&tmp, path)
        };
        if let Err(err) = write() {
            tracing::warn!(path = %path.display(), %err, "could not save update state");
        }
    }

    /// Whether the last answer is recent enough to use instead of asking.
    fn fresh(&self, now: u64) -> bool {
        self.last_check
            .is_some_and(|at| now.saturating_sub(at) < CHECK_INTERVAL.as_secs())
    }

    /// The release to tell the user about at launch, if any: newer than
    /// `current` and not skipped.
    fn launch_notice(&self, current: Version) -> Option<&Release> {
        self.latest.as_ref().filter(|release| {
            is_newer(release, current) && self.skipped.as_deref() != Some(&release.version)
        })
    }
}

fn is_newer(release: &Release, current: Version) -> bool {
    Version::parse(&release.version).is_some_and(|v| v > current)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Asks GitHub for the latest release. Blocks, so it runs on its own thread.
fn fetch() -> Result<Release, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .into();
    let json = agent
        .get(LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", concat!("rusting/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|err| err.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|err| err.to_string())?;
    parse_release(&json).ok_or_else(|| "GitHub's answer had no release in it".to_owned())
}

/// A request on its way.
struct Pending {
    answer: mpsc::Receiver<Result<Release, String>>,
    /// From Help → Check for Updates…: always report the outcome.
    manual: bool,
}

/// Finds newer releases and shows the update dialog.
pub struct UpdateChecker {
    state_path: Option<PathBuf>,
    state: State,
    current: Option<Version>,
    pending: Option<Pending>,
    notice: Option<ui::UpdateNotice>,
    /// Wakes the event loop when an answer arrives.
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl UpdateChecker {
    /// A checker that calls `wake` (from another thread) when GitHub answers.
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let state_path = crate::layout_file::config_dir().map(|dir| dir.join(FILE_NAME));
        let state = state_path.as_deref().map(State::load).unwrap_or_default();
        Self {
            state_path,
            state,
            current: Version::current(),
            pending: None,
            notice: None,
            wake: Arc::new(wake),
        }
    }

    /// The launch check: shows the release found within the last day, or
    /// else asks GitHub. Does nothing when turned off.
    pub fn on_launch(&mut self) {
        let off = std::env::var_os("RUSTING_NO_UPDATE_CHECK").is_some_and(|v| v != "0");
        let Some(current) = self.current else {
            return;
        };
        if off || !self.state.enabled {
            return;
        }
        if self.state.fresh(now()) {
            if let Some(release) = self.state.launch_notice(current).cloned() {
                self.show(release);
            }
        } else {
            self.start(false);
        }
    }

    /// Help → Check for Updates…: asks GitHub now, and reports the answer
    /// even if the release was skipped or the program is up to date.
    pub fn check_now(&mut self, session: &mut ui::Session) {
        match &mut self.pending {
            Some(pending) => pending.manual = true,
            None => self.start(true),
        }
        session.set_status("Checking for updates…");
    }

    fn start(&mut self, manual: bool) {
        let (send, answer) = mpsc::channel();
        let wake = Arc::clone(&self.wake);
        let spawned = std::thread::Builder::new()
            .name("update-check".into())
            .spawn(move || {
                let _ = send.send(fetch());
                wake();
            });
        match spawned {
            Ok(_) => self.pending = Some(Pending { answer, manual }),
            Err(err) => tracing::warn!(%err, "could not start the update check"),
        }
    }

    fn show(&mut self, release: Release) {
        let Some(current) = self.current else {
            return;
        };
        self.notice = Some(ui::UpdateNotice {
            version: release.version,
            current: format!("{}.{}.{}", current.0, current.1, current.2),
            summary: release.summary,
            url: release.url,
            homebrew: running_from_homebrew(),
            check_on_launch: self.state.enabled,
        });
    }

    fn save(&self) {
        if let Some(path) = &self.state_path {
            self.state.save(path);
        }
    }

    /// Takes GitHub's answer if it has arrived.
    fn receive(&mut self, session: &mut ui::Session) {
        let Some(pending) = &self.pending else {
            return;
        };
        let answer = match pending.answer.try_recv() {
            Ok(answer) => answer,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("the check stopped".to_owned()),
        };
        let manual = pending.manual;
        self.pending = None;
        let Some(current) = self.current else {
            return;
        };
        match answer {
            Ok(release) => {
                self.state.last_check = Some(now());
                self.state.latest = Some(release.clone());
                self.save();
                let newer = is_newer(&release, current);
                let skipped = self.state.skipped.as_deref() == Some(&release.version);
                if newer && (manual || !skipped) {
                    self.show(release);
                } else if manual {
                    session.set_status(format!(
                        "Rusting {}.{}.{} is the latest version.",
                        current.0, current.1, current.2
                    ));
                }
            }
            Err(err) => {
                tracing::info!(%err, "update check failed");
                if manual {
                    session.set_error(format!("Could not check for updates: {err}"));
                }
            }
        }
    }

    /// Takes GitHub's answer when it arrives and shows the dialog, unless
    /// another dialog (`blocked`) is up.
    pub fn ui(&mut self, ctx: &egui::Context, session: &mut ui::Session, blocked: bool) {
        self.receive(session);
        if blocked {
            return;
        }
        let Some(notice) = &mut self.notice else {
            return;
        };
        let Some(choice) = ui::update_dialog(ctx, notice) else {
            return;
        };
        let notice = self.notice.take();
        if let Some(notice) = notice {
            self.state.enabled = notice.check_on_launch;
            match choice {
                ui::UpdateChoice::OpenPage => ctx.open_url(egui::OpenUrl::new_tab(notice.url)),
                ui::UpdateChoice::Later => {}
                ui::UpdateChoice::Skip => self.state.skipped = Some(notice.version),
            }
        }
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_compare() {
        assert_eq!(Version::parse("0.2.3"), Some(Version(0, 2, 3)));
        assert_eq!(Version::parse("v1.10.0"), Some(Version(1, 10, 0)));
        assert_eq!(Version::parse("0.3.0-beta.1"), Some(Version(0, 3, 0)));
        assert_eq!(Version::parse("0.2"), None);
        assert_eq!(Version::parse("0.2.3.4"), None);
        assert_eq!(Version::parse("latest"), None);
        assert!(Version(0, 10, 0) > Version(0, 9, 9));
        assert!(Version::current().is_some());
    }

    fn github_json(tag: &str, body: &str, prerelease: bool) -> String {
        serde_json::json!({
            "tag_name": tag,
            "html_url": format!("https://github.com/HalfdanIsaksen/rusting/releases/tag/{tag}"),
            "body": body,
            "draft": false,
            "prerelease": prerelease,
        })
        .to_string()
    }

    #[test]
    fn reads_githubs_release_json() {
        let json = github_json(
            "v0.3.0",
            "Adds frame tags.\r\n\r\n### New\r\n- **Tags**: name frames.",
            false,
        );
        let release = parse_release(&json).expect("release");
        assert_eq!(release.version, "0.3.0");
        assert!(release.url.ends_with("/v0.3.0"));
        assert_eq!(release.summary.as_deref(), Some("Adds frame tags."));
    }

    #[test]
    fn skips_pre_releases_and_bad_json() {
        assert_eq!(parse_release(&github_json("v0.3.0", "", true)), None);
        assert_eq!(parse_release(&github_json("nightly", "", false)), None);
        assert_eq!(parse_release("{}"), None);
        assert_eq!(parse_release("not json"), None);
    }

    #[test]
    fn summary_skips_headings_and_lists() {
        assert_eq!(summary("### New\n- **A**: b\n"), None);
        assert_eq!(
            summary("\n## What's Changed\nFixes saving.\n").as_deref(),
            Some("Fixes saving.")
        );
        let long = "a".repeat(400);
        let text = summary(&long).expect("summary");
        assert_eq!(text.chars().count(), MAX_SUMMARY + 1);
        assert!(text.ends_with('…'));
    }

    #[test]
    fn detects_homebrew_installs() {
        assert!(is_homebrew(Path::new(
            "/opt/homebrew/Cellar/rusting/0.2.2/bin/rusting"
        )));
        assert!(is_homebrew(Path::new(
            "/usr/local/Cellar/rusting/0.2.2/bin/rusting"
        )));
        assert!(is_homebrew(Path::new(
            "/home/linuxbrew/.linuxbrew/bin/rusting"
        )));
        assert!(!is_homebrew(Path::new(
            "/Applications/Rusting.app/Contents/MacOS/rusting"
        )));
    }

    fn release(version: &str) -> Release {
        Release {
            version: version.into(),
            url: String::new(),
            summary: None,
        }
    }

    #[test]
    fn checks_at_most_once_a_day() {
        let day = CHECK_INTERVAL.as_secs();
        let mut state = State::default();
        assert!(!state.fresh(1_000_000));
        state.last_check = Some(1_000_000);
        assert!(state.fresh(1_000_000 + day - 1));
        assert!(!state.fresh(1_000_000 + day));
    }

    #[test]
    fn launch_notice_needs_a_newer_unskipped_release() {
        let current = Version(0, 2, 2);
        let mut state = State {
            latest: Some(release("0.2.2")),
            ..State::default()
        };
        assert_eq!(state.launch_notice(current), None);
        state.latest = Some(release("0.3.0"));
        assert_eq!(state.launch_notice(current), Some(&release("0.3.0")));
        state.skipped = Some("0.3.0".into());
        assert_eq!(state.launch_notice(current), None);
        // A release after the skipped one asks again.
        state.latest = Some(release("0.3.1"));
        assert!(state.launch_notice(current).is_some());
    }

    #[test]
    fn state_round_trips_and_defaults_to_on() {
        let dir = std::env::temp_dir().join(format!("rusting-update-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(FILE_NAME);
        assert_eq!(State::load(&path), State::default());
        assert!(State::default().enabled);
        let state = State {
            enabled: false,
            last_check: Some(42),
            latest: Some(release("0.3.0")),
            skipped: Some("0.3.0".into()),
        };
        state.save(&path);
        assert_eq!(State::load(&path), state);
        std::fs::write(&path, "{ broken").expect("write");
        assert_eq!(State::load(&path), State::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
