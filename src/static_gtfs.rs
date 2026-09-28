use crate::orchestrator::StaticSnapshot;
use crate::static_augment::{augment_static, PlatformTable};
use async_trait::async_trait;
use gtfs_structures::Gtfs;
use serde_json::Value;
use std::io::Cursor;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

static SNAPSHOT_COUNTER: AtomicU64 = AtomicU64::new(0);
static VALIDATOR_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Static-fetch, parse, or staging failure that leaves active state unchanged.
#[derive(Debug)]
pub struct StaticError(pub String);

impl std::fmt::Display for StaticError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for StaticError {}

/// Standards-validator failure that rejects a staged static candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaticValidationError(pub String);

impl std::fmt::Display for StaticValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for StaticValidationError {}

/// Independent static-GTFS standards gate for the exact fetched ZIP bytes.
#[async_trait]
pub trait StaticStandardsValidator: Send + Sync {
    async fn validate(&self, zip: Arc<[u8]>) -> Result<(), StaticValidationError>;
}

/// Production MobilityData GTFS validator adapter.
///
/// Task 1.1 already verifies that `jar` is the pinned `8.0.1` artifact and
/// Java 17+ is usable. Each invocation is still bounded and requires a
/// well-formed report containing zero `ERROR` notices.
pub struct MobilityDataStaticValidator {
    jar: PathBuf,
    timeout: Duration,
}

impl MobilityDataStaticValidator {
    /// Creates a bounded adapter for the startup-verified validator artifact.
    ///
    /// The constructor deliberately does not repeat task 1.1's checksum and
    /// Java probes; callers must pass only the path from `ValidatedConfig`.
    pub fn new(jar: PathBuf) -> Self {
        Self {
            jar,
            timeout: Duration::from_secs(60),
        }
    }

    #[cfg(test)]
    fn with_timeout(jar: PathBuf, timeout: Duration) -> Self {
        Self { jar, timeout }
    }
}

#[async_trait]
impl StaticStandardsValidator for MobilityDataStaticValidator {
    async fn validate(&self, zip: Arc<[u8]>) -> Result<(), StaticValidationError> {
        validate_with_mobility_data(&self.jar, &zip, self.timeout).await
    }
}

/// Fetches a static ZIP once, validates, then retains and parses those bytes.
///
/// With `platforms`, the upstream ZIP is augmented with platform stops first, and the augmented
/// bytes are the ones parsed, validated, and returned. If augmentation fails or the validator
/// rejects the augmented bytes, the upstream bytes are parsed and validated from scratch and
/// returned instead, so enabling tracks can never block static publication.
///
/// # Errors
///
/// Returns [`StaticError`] when the request, HTTP status, body read, GTFS parse,
/// or standards gate fails. The validator sees the same retained buffer that is
/// parsed and returned; no independently refetched bytes can become active.
pub async fn fetch_static(
    url: &str,
    validator: &dyn StaticStandardsValidator,
    platforms: Option<&PlatformTable>,
) -> Result<StaticSnapshot, StaticError> {
    fetch_static_at(url, validator, platforms, SystemTime::now()).await
}

async fn fetch_static_at(
    url: &str,
    validator: &dyn StaticStandardsValidator,
    platforms: Option<&PlatformTable>,
    now: SystemTime,
) -> Result<StaticSnapshot, StaticError> {
    let response = reqwest::get(url)
        .await
        .map_err(|_| StaticError("static fetch failed".into()))?
        .error_for_status()
        .map_err(|_| StaticError("static fetch status failed".into()))?;
    let bytes = response
        .bytes()
        .await
        .map_err(|_| StaticError("static body failed".into()))?;
    if let Some(table) = platforms {
        if let Some(snapshot) = augmented_snapshot(bytes.as_ref(), table, validator, now).await {
            return Ok(snapshot);
        }
    }
    let zip: Arc<[u8]> = Arc::from(bytes.as_ref());
    let snapshot = snapshot_from_bytes(zip.clone(), now, None)?;
    validator
        .validate(zip)
        .await
        .map_err(|_| StaticError("static standards validation failed".into()))?;
    Ok(snapshot)
}

/// Augmented, parsed, and validated snapshot, or `None` to publish upstream bytes instead.
async fn augmented_snapshot(
    upstream: &[u8],
    table: &PlatformTable,
    validator: &dyn StaticStandardsValidator,
    now: SystemTime,
) -> Option<StaticSnapshot> {
    let augmented = match augment_static(upstream, table) {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(%error, stage = "static", "platform augmentation failed; publishing upstream");
            return None;
        }
    };
    let zip: Arc<[u8]> = Arc::from(augmented);
    let suffix = format!("+tracks.{}", table.digest());
    let Ok(snapshot) = snapshot_from_bytes(zip.clone(), now, Some(&suffix)) else {
        tracing::warn!(
            stage = "static",
            "augmented static did not parse; publishing upstream"
        );
        return None;
    };
    if validator.validate(zip).await.is_err() {
        tracing::warn!(
            stage = "static",
            "augmented static rejected by validator; publishing upstream"
        );
        return None;
    }
    Some(snapshot)
}

/// Parses retained bytes into a snapshot whose version is `feed_version` plus `suffix`.
///
/// Augmentation leaves `feed_info.txt` untouched, so the `+tracks.{digest}` suffix is applied here
/// rather than read from the feed; versionless feeds get a generated identifier first.
fn snapshot_from_bytes(
    zip: Arc<[u8]>,
    now: SystemTime,
    suffix: Option<&str>,
) -> Result<StaticSnapshot, StaticError> {
    let parsed = Gtfs::from_reader(Cursor::new(zip.clone()))
        .map_err(|_| StaticError("static parse failed".into()))?;
    let version = parsed
        .feed_info
        .iter()
        .filter_map(|info| info.version.as_deref())
        .find(|version| !version.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| fallback_snapshot_version(now));
    Ok(StaticSnapshot {
        version: format!("{version}{}", suffix.unwrap_or_default()),
        parsed: Arc::new(parsed),
        zip,
    })
}

/// Reconstructs active static state from a previously validated durable generation.
///
/// Recovery parses the exact retained ZIP but preserves the committed manifest
/// version, including the generated identifier used for versionless feeds. It
/// performs no network access, allowing the service to serve last-good data and
/// resume realtime polling while the upstream static endpoint is unavailable.
///
/// # Errors
///
/// Returns [`StaticError`] when the committed identifier is empty or the
/// retained ZIP can no longer be parsed.
pub fn recover_static(
    zip: Arc<[u8]>,
    committed_version: String,
) -> Result<StaticSnapshotState, StaticError> {
    if committed_version.trim().is_empty() {
        return Err(StaticError("recovered static version is empty".into()));
    }
    let parsed = Gtfs::from_reader(Cursor::new(zip.clone()))
        .map_err(|_| StaticError("recovered static parse failed".into()))?;
    Ok(StaticSnapshotState::new(Arc::new(StaticSnapshot {
        version: committed_version,
        parsed: Arc::new(parsed),
        zip,
    })))
}

fn fallback_snapshot_version(now: SystemTime) -> String {
    let nanoseconds = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = SNAPSHOT_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("snapshot-{nanoseconds}-{counter}")
}

/// Fetches and validates a replacement without switching active static state.
///
/// Byte-identical data returns `None`; changed data is returned only after the
/// configured standards validator accepts the exact retained response bytes.
///
/// # Errors
///
/// Returns [`StaticError`] for fetch, parse, standards-error, malformed-report,
/// timeout, or validator-tool failure. The caller's active snapshot is never
/// mutated by this function.
pub async fn stage_static(
    comparison: &StaticSnapshot,
    url: &str,
    validator: &dyn StaticStandardsValidator,
    platforms: Option<&PlatformTable>,
) -> Result<Option<Arc<StaticSnapshot>>, StaticError> {
    let candidate = Arc::new(fetch_static(url, validator, platforms).await?);
    if candidate.zip.as_ref() == comparison.zip.as_ref() {
        return Ok(None);
    }
    Ok(Some(candidate))
}

#[derive(Clone)]
struct SnapshotPair {
    active: Arc<StaticSnapshot>,
    pending: Option<Arc<StaticSnapshot>>,
}

/// Active and pending snapshots. Pending can become active only after its
/// complete static/realtime generation has durably committed.
#[derive(Clone)]
pub struct StaticSnapshotState {
    inner: Arc<RwLock<SnapshotPair>>,
}

impl StaticSnapshotState {
    /// Starts internal lifecycle management with one already accepted snapshot.
    ///
    /// Production startup should use [`bootstrap_static`] for newly fetched
    /// data. This constructor is crate-scoped because recovered generations and
    /// tests already carry acceptance evidence and must not refetch the ZIP.
    pub(crate) fn new(active: Arc<StaticSnapshot>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(SnapshotPair {
                active,
                pending: None,
            })),
        }
    }

    /// Returns the snapshot paired with the currently published generation.
    pub async fn active(&self) -> Arc<StaticSnapshot> {
        self.inner.read().await.active.clone()
    }

    /// Returns the accepted replacement awaiting a coherent realtime commit.
    pub async fn pending(&self) -> Option<Arc<StaticSnapshot>> {
        self.inner.read().await.pending.clone()
    }

    /// Selects pending-first input and reports whether commit should promote it.
    pub async fn candidate(&self) -> (Arc<StaticSnapshot>, bool) {
        let state = self.inner.read().await;
        state.pending.as_ref().map_or_else(
            || (state.active.clone(), false),
            |value| (value.clone(), true),
        )
    }

    /// Replaces the pending candidate without changing active static state.
    pub async fn stage(&self, snapshot: Arc<StaticSnapshot>) {
        self.inner.write().await.pending = Some(snapshot);
    }

    /// Promotes exactly the snapshot that was committed. A newer concurrently
    /// staged snapshot remains pending for the next refresh attempt.
    pub async fn promote_committed(&self, snapshot: &Arc<StaticSnapshot>) {
        let mut state = self.inner.write().await;
        state.active = snapshot.clone();
        if state
            .pending
            .as_ref()
            .is_some_and(|pending| Arc::ptr_eq(pending, snapshot))
        {
            state.pending = None;
        }
    }
}

/// Fetches and standards-validates the initial active static snapshot.
///
/// This is the production bootstrap boundary for a newly downloaded schedule;
/// it prevents initial startup from bypassing the same exact-byte gate applied
/// to later replacements.
///
/// # Errors
///
/// Returns [`StaticError`] without constructing active state when fetching,
/// parsing, or standards validation fails.
pub async fn bootstrap_static(
    static_url: &str,
    validator: &dyn StaticStandardsValidator,
    platforms: Option<&PlatformTable>,
) -> Result<StaticSnapshotState, StaticError> {
    let snapshot = fetch_static(static_url, validator, platforms).await?;
    Ok(StaticSnapshotState::new(Arc::new(snapshot)))
}

/// Credential-free result of one scheduled static refresh attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StaticRefreshOutcome {
    Staged(String),
    Unchanged,
    Rejected,
}

/// Attempts one static replacement and stages it without publishing it.
///
/// Failures are intentionally summarized rather than returning or logging raw
/// request/tool errors, which could contain configured URLs or credentials.
pub async fn refresh_snapshot_once(
    state: &StaticSnapshotState,
    static_url: &str,
    validator: &dyn StaticStandardsValidator,
    platforms: Option<&PlatformTable>,
) -> StaticRefreshOutcome {
    let comparison = match state.pending().await {
        Some(pending) => pending,
        None => state.active().await,
    };
    match stage_static(&comparison, static_url, validator, platforms).await {
        Ok(Some(snapshot)) => {
            let version = snapshot.version.clone();
            state.stage(snapshot).await;
            StaticRefreshOutcome::Staged(version)
        }
        Ok(None) => StaticRefreshOutcome::Unchanged,
        Err(_) => StaticRefreshOutcome::Rejected,
    }
}

/// Periodically stages standards-valid replacements without publishing them.
pub async fn run_snapshot_refresh(
    state: StaticSnapshotState,
    static_url: String,
    interval: Duration,
    validator: Arc<dyn StaticStandardsValidator>,
    platforms: Option<Arc<PlatformTable>>,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.tick().await;
    loop {
        ticker.tick().await;
        match refresh_snapshot_once(
            &state,
            &static_url,
            validator.as_ref(),
            platforms.as_deref(),
        )
        .await
        {
            StaticRefreshOutcome::Staged(version) => {
                tracing::info!(
                    outcome = "staged",
                    stage = "static",
                    static_version = %version,
                    "static refresh"
                );
            }
            StaticRefreshOutcome::Unchanged => {
                tracing::info!(outcome = "unchanged", stage = "static", "static refresh")
            }
            StaticRefreshOutcome::Rejected => {
                tracing::warn!(outcome = "rejected", stage = "static", "static refresh")
            }
        }
    }
}

async fn validate_with_mobility_data(
    jar: &Path,
    zip: &[u8],
    timeout: Duration,
) -> Result<(), StaticValidationError> {
    let unique = VALIDATOR_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = std::env::temp_dir().join(format!(
        "amtrak-static-validator-{}-{unique}",
        std::process::id()
    ));
    let report_dir = temporary.join("report");
    std::fs::create_dir(&temporary)
        .and_then(|_| make_validator_directory_private(&temporary))
        .and_then(|_| std::fs::create_dir(&report_dir))
        .and_then(|_| make_validator_directory_private(&report_dir))
        .map_err(|error| StaticValidationError(format!("validator temp setup failed: {error}")))?;
    let _cleanup = TemporaryDirectoryGuard(temporary.clone());
    let zip_path = temporary.join("static.zip");
    async {
        let mut input = validator_input_file(&zip_path)
            .map_err(|error| StaticValidationError(format!("validator input failed: {error}")))?;
        input
            .write_all(zip)
            .and_then(|_| input.sync_all())
            .map_err(|error| StaticValidationError(format!("validator input failed: {error}")))?;
        let mut command = Command::new("java");
        command
            .arg("-jar")
            .arg(jar)
            .arg("-i")
            .arg(&zip_path)
            .arg("-o")
            .arg(&report_dir)
            .arg("--skip_validator_update")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        run_command_bounded(&mut command, timeout).await?;
        let report = std::fs::read(report_dir.join("report.json"))
            .map_err(|error| StaticValidationError(format!("validator report missing: {error}")))?;
        validate_report(&report)
    }
    .await
}

struct TemporaryDirectoryGuard(PathBuf);

impl Drop for TemporaryDirectoryGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
fn make_validator_directory_private(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn make_validator_directory_private(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn validator_input_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

struct ChildGuard(Option<std::process::Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

async fn run_command_bounded(
    command: &mut Command,
    timeout: Duration,
) -> Result<(), StaticValidationError> {
    let mut child = ChildGuard(Some(command.spawn().map_err(|error| {
        StaticValidationError(format!("validator could not start: {error}"))
    })?));
    let deadline = Instant::now() + timeout;
    loop {
        match child.0.as_mut().expect("child remains owned").try_wait() {
            Ok(Some(status)) if status.success() => {
                child.0.take();
                return Ok(());
            }
            Ok(Some(_)) => {
                child.0.take();
                return Err(StaticValidationError(
                    "validator exited unsuccessfully".into(),
                ));
            }
            Ok(None) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
            Ok(None) => {
                return Err(StaticValidationError("validator timed out".into()));
            }
            Err(error) => {
                return Err(StaticValidationError(format!(
                    "validator status failed: {error}"
                )))
            }
        }
    }
}

fn validate_report(report: &[u8]) -> Result<(), StaticValidationError> {
    let value: Value = serde_json::from_slice(report)
        .map_err(|error| StaticValidationError(format!("malformed validator report: {error}")))?;
    let notices = value
        .get("notices")
        .and_then(Value::as_array)
        .ok_or_else(|| StaticValidationError("validator report has no notices array".into()))?;
    let mut errors = 0usize;
    for notice in notices {
        let severity = notice
            .get("severity")
            .and_then(Value::as_str)
            .ok_or_else(|| StaticValidationError("validator notice has no severity".into()))?;
        if severity == "ERROR" {
            errors += 1;
        }
    }
    if errors == 0 {
        Ok(())
    } else {
        Err(StaticValidationError(format!(
            "validator reported {errors} ERROR notice(s)"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use zip::write::SimpleFileOptions;

    struct RecordingValidator {
        seen: Mutex<Vec<Arc<[u8]>>>,
        result: Result<(), StaticValidationError>,
    }

    #[async_trait]
    impl StaticStandardsValidator for RecordingValidator {
        async fn validate(&self, zip: Arc<[u8]>) -> Result<(), StaticValidationError> {
            self.seen.lock().unwrap().push(zip);
            self.result.clone()
        }
    }

    fn fixture_zip(version: Option<&str>) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default();
        let files = [
            ("agency.txt", "agency_id,agency_name,agency_url,agency_timezone\na,Amtrak,https://amtrak.com,America/New_York\n"),
            ("stops.txt", "stop_id,stop_name,stop_lat,stop_lon\ns,Station,42,-71\n"),
            ("routes.txt", "route_id,agency_id,route_short_name,route_long_name,route_type\nr,a,R,Regional,2\n"),
            ("trips.txt", "route_id,service_id,trip_id\nr,svc,t\n"),
            ("stop_times.txt", "trip_id,arrival_time,departure_time,stop_id,stop_sequence\nt,10:00:00,10:00:00,s,1\n"),
            ("calendar_dates.txt", "service_id,date,exception_type\nsvc,20260813,1\n"),
        ];
        for (name, contents) in files {
            archive.start_file(name, options).unwrap();
            archive.write_all(contents.as_bytes()).unwrap();
        }
        if let Some(version) = version {
            archive.start_file("feed_info.txt", options).unwrap();
            archive
                .write_all(format!("feed_publisher_name,feed_publisher_url,feed_lang,feed_version\nAmtrak,https://amtrak.com,en,{version}\n").as_bytes())
                .unwrap();
        }
        archive.finish().unwrap().into_inner()
    }

    #[test]
    fn exact_zip_is_parsed_and_versionless_snapshots_are_identifying() {
        let bytes = fixture_zip(Some("V1"));
        let snapshot = snapshot_from_bytes(
            Arc::from(bytes.clone()),
            UNIX_EPOCH + Duration::from_secs(1),
            None,
        )
        .unwrap();
        assert_eq!(snapshot.zip.as_ref(), bytes);
        assert_eq!(snapshot.version, "V1");
        assert!(snapshot.parsed.trips.contains_key("t"));

        let bytes = fixture_zip(None);
        let first = snapshot_from_bytes(
            Arc::from(bytes.clone()),
            UNIX_EPOCH + Duration::from_secs(2),
            None,
        )
        .unwrap();
        let second =
            snapshot_from_bytes(Arc::from(bytes), UNIX_EPOCH + Duration::from_secs(2), None)
                .unwrap();
        assert_ne!(first.version, second.version);
        assert!(first.version.starts_with("snapshot-"));
        assert_ne!(first.version, "unknown");
    }

    #[test]
    fn validator_report_requires_zero_errors_and_well_formed_notices() {
        assert!(validate_report(br#"{"notices":[]}"#).is_ok());
        assert!(validate_report(br#"{"notices":[{"severity":"WARNING"}]}"#).is_ok());
        assert!(validate_report(br#"{"notices":[{"severity":"ERROR"}]}"#).is_err());
        assert!(validate_report(br#"{}"#).is_err());
        assert!(validate_report(b"not-json").is_err());
    }

    #[tokio::test]
    async fn staging_fetches_once_and_validates_the_exact_retained_bytes() {
        let bytes = fixture_zip(Some("NEW"));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let server_requests = requests.clone();
        let response_bytes = bytes.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            server_requests.fetch_add(1, Ordering::Relaxed);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_bytes.len()
            );
            stream.write_all(header.as_bytes()).await.unwrap();
            stream.write_all(&response_bytes).await.unwrap();
        });

        let current =
            snapshot_from_bytes(Arc::from(fixture_zip(Some("OLD"))), UNIX_EPOCH, None).unwrap();
        let validator = RecordingValidator {
            seen: Mutex::new(Vec::new()),
            result: Ok(()),
        };
        let staged = stage_static(
            &current,
            &format!("http://{address}/static.zip"),
            &validator,
            None,
        )
        .await
        .unwrap()
        .unwrap();
        server.await.unwrap();

        assert_eq!(requests.load(Ordering::Relaxed), 1);
        assert_eq!(staged.zip.as_ref(), bytes);
        let seen = validator.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].as_ref(), staged.zip.as_ref());
    }

    #[tokio::test]
    async fn bootstrap_rejection_cannot_initialize_active_static_state() {
        let bytes = fixture_zip(Some("INITIAL"));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let expected = bytes.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            stream.write_all(header.as_bytes()).await.unwrap();
            stream.write_all(&bytes).await.unwrap();
        });
        let validator = RecordingValidator {
            seen: Mutex::new(Vec::new()),
            result: Err(StaticValidationError("fixture error".into())),
        };

        let result =
            bootstrap_static(&format!("http://{address}/static.zip"), &validator, None).await;
        server.await.unwrap();

        assert!(result.is_err());
        let seen = validator.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].as_ref(), expected);
    }

    #[tokio::test]
    async fn validator_tool_failure_is_actionable() {
        let validator = MobilityDataStaticValidator::with_timeout(
            PathBuf::from("/definitely/missing/validator.jar"),
            Duration::from_secs(1),
        );
        assert!(validator.validate(Arc::from(&b"zip"[..])).await.is_err());
    }

    #[tokio::test]
    async fn validator_subprocess_timeout_is_bounded() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 2"]);
        let error = run_command_bounded(&mut command, Duration::from_millis(10))
            .await
            .unwrap_err();
        assert!(error.0.contains("timed out"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelling_validator_future_kills_and_reaps_child_before_join() {
        let marker = std::env::temp_dir().join(format!(
            "amtrak-validator-child-{}-{}",
            std::process::id(),
            VALIDATOR_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let command_text = format!("echo $$ > '{}'; sleep 30", marker.display());
        let task = tokio::spawn(async move {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", &command_text]);
            run_command_bounded(&mut command, Duration::from_secs(60)).await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !marker.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let pid = std::fs::read_to_string(&marker).unwrap();

        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let status = Command::new("kill")
            .args(["-0", pid.trim()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(!status.success(), "validator child still exists after join");
        std::fs::remove_file(marker).unwrap();
    }

    #[tokio::test]
    async fn snapshot_state_promotes_only_the_committed_pending_value() {
        let active = Arc::new(
            snapshot_from_bytes(Arc::from(fixture_zip(Some("A"))), UNIX_EPOCH, None).unwrap(),
        );
        let first = Arc::new(
            snapshot_from_bytes(Arc::from(fixture_zip(Some("B"))), UNIX_EPOCH, None).unwrap(),
        );
        let newer = Arc::new(
            snapshot_from_bytes(Arc::from(fixture_zip(Some("C"))), UNIX_EPOCH, None).unwrap(),
        );
        let state = StaticSnapshotState::new(active);
        state.stage(first.clone()).await;
        state.stage(newer.clone()).await;
        state.promote_committed(&first).await;
        assert_eq!(state.active().await.version, "B");
        assert_eq!(state.pending().await.unwrap().version, "C");
        state.promote_committed(&newer).await;
        assert_eq!(state.active().await.version, "C");
        assert!(state.pending().await.is_none());
    }

    #[tokio::test]
    async fn rejected_static_refresh_preserves_active_and_pending_snapshots() {
        let active = Arc::new(
            snapshot_from_bytes(Arc::from(fixture_zip(Some("A"))), UNIX_EPOCH, None).unwrap(),
        );
        let pending = Arc::new(
            snapshot_from_bytes(Arc::from(fixture_zip(Some("B"))), UNIX_EPOCH, None).unwrap(),
        );
        let state = StaticSnapshotState::new(active.clone());
        state.stage(pending.clone()).await;

        let bytes = fixture_zip(Some("C"));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            stream.write_all(header.as_bytes()).await.unwrap();
            stream.write_all(&bytes).await.unwrap();
        });
        let validator = RecordingValidator {
            seen: Mutex::new(Vec::new()),
            result: Err(StaticValidationError("fixture error".into())),
        };

        let result = refresh_snapshot_once(
            &state,
            &format!("http://{address}/static.zip"),
            &validator,
            None,
        )
        .await;
        server.await.unwrap();

        assert_eq!(result, StaticRefreshOutcome::Rejected);
        assert!(Arc::ptr_eq(&state.active().await, &active));
        assert!(Arc::ptr_eq(&state.pending().await.unwrap(), &pending));
        assert_eq!(validator.seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn retained_static_recovers_without_network_or_new_version() {
        let zip: Arc<[u8]> = Arc::from(fixture_zip(None));
        let state = recover_static(zip.clone(), "committed-fallback".into()).unwrap();
        let active = state.active().await;
        assert_eq!(active.version, "committed-fallback");
        assert_eq!(active.zip.as_ref(), zip.as_ref());
        assert!(active.parsed.trips.contains_key("t"));
    }

    fn served_zip(bytes: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request).await;
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                );
                let _ = stream.write_all(header.as_bytes()).await;
                let _ = stream.write_all(&bytes).await;
            }
        });
        (format!("http://{address}/static.zip"), server)
    }

    fn augmentable_zip(version: &str) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default();
        let files = [
            ("agency.txt", "agency_id,agency_name,agency_url,agency_timezone\na,Amtrak,https://amtrak.com,America/New_York\n"),
            ("stops.txt", "stop_id,stop_name,stop_lat,stop_lon\nNWK,Newark,40.7,-74.1\n"),
            ("routes.txt", "route_id,agency_id,route_short_name,route_long_name,route_type\nr,a,R,Regional,2\n"),
            ("trips.txt", "route_id,service_id,trip_id\nr,svc,t\n"),
            ("stop_times.txt", "trip_id,arrival_time,departure_time,stop_id,stop_sequence\nt,10:00:00,10:00:00,NWK,1\n"),
            ("calendar_dates.txt", "service_id,date,exception_type\nsvc,20260813,1\n"),
        ];
        for (name, contents) in files {
            archive.start_file(name, options).unwrap();
            archive.write_all(contents.as_bytes()).unwrap();
        }
        archive.start_file("feed_info.txt", options).unwrap();
        archive
            .write_all(format!("feed_publisher_name,feed_publisher_url,feed_lang,feed_version\nAmtrak,https://amtrak.com,en,{version}\n").as_bytes())
            .unwrap();
        archive.finish().unwrap().into_inner()
    }

    /// Accepts only archives without platform stops, to drive the fallback path.
    struct RejectAugmented {
        seen: Mutex<Vec<Arc<[u8]>>>,
    }

    #[async_trait]
    impl StaticStandardsValidator for RejectAugmented {
        async fn validate(&self, zip: Arc<[u8]>) -> Result<(), StaticValidationError> {
            self.seen.lock().unwrap().push(zip.clone());
            let parsed = Gtfs::from_reader(Cursor::new(zip)).unwrap();
            if parsed.stops.keys().any(|id| id.contains(":track:")) {
                Err(StaticValidationError("fixture rejects platforms".into()))
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn augmented_bytes_are_validated_published_and_versioned() {
        let upstream = augmentable_zip("20260927");
        let (url, server) = served_zip(upstream.clone());
        let validator = RecordingValidator {
            seen: Mutex::new(Vec::new()),
            result: Ok(()),
        };
        let table = PlatformTable::parse("NWK=1-2").unwrap();
        let snapshot = fetch_static(&url, &validator, Some(&table)).await.unwrap();
        assert_eq!(
            snapshot.version,
            format!("20260927+tracks.{}", table.digest())
        );
        assert!(snapshot.parsed.stops.contains_key("NWK:track:2"));
        assert_ne!(snapshot.zip.as_ref(), upstream.as_slice());
        {
            let seen = validator.seen.lock().unwrap();
            assert_eq!(seen.len(), 1);
            assert_eq!(seen[0].as_ref(), snapshot.zip.as_ref());
        }

        let other = PlatformTable::parse("NWK=1-3").unwrap();
        let changed = fetch_static(&url, &validator, Some(&other)).await.unwrap();
        assert_ne!(changed.version, snapshot.version);
        let same = fetch_static(&url, &validator, Some(&table)).await.unwrap();
        assert_eq!(same.version, snapshot.version);
        assert_eq!(same.zip.as_ref(), snapshot.zip.as_ref());

        let disabled = fetch_static(&url, &validator, None).await.unwrap();
        assert_eq!(disabled.version, "20260927");
        assert_eq!(disabled.zip.as_ref(), upstream.as_slice());
        server.abort();
    }

    #[tokio::test]
    async fn rejected_or_failed_augmentation_publishes_upstream_bytes() {
        let upstream = augmentable_zip("20260927");
        let (url, server) = served_zip(upstream.clone());
        let validator = RejectAugmented {
            seen: Mutex::new(Vec::new()),
        };
        let table = PlatformTable::parse("NWK=1").unwrap();
        let snapshot = fetch_static(&url, &validator, Some(&table)).await.unwrap();
        assert_eq!(snapshot.zip.as_ref(), upstream.as_slice());
        assert_eq!(snapshot.version, "20260927");
        assert!(!snapshot.parsed.stops.contains_key("NWK:track:1"));
        assert_eq!(validator.seen.lock().unwrap().len(), 2);

        // A generated id colliding with an existing stop fails augmentation before validation.
        let colliding = PlatformTable::parse("NWK=1").unwrap();
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        archive.start_file("stops.txt", options).unwrap();
        archive
            .write_all(
                b"stop_id,stop_name,stop_lat,stop_lon\nNWK,Newark,40,-74\nNWK:station,X,40,-74\n",
            )
            .unwrap();
        let broken = archive.finish().unwrap().into_inner();
        assert!(augment_static(&broken, &colliding).is_err());
        server.abort();
    }
}
