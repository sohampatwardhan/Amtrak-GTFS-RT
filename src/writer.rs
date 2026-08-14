use crate::orchestrator::{
    ArtifactUrls, EntityCounts, FeedSetManifest, GenerationId, ValidatedGeneration,
};
use gtfs_realtime::FeedMessage;
use prost::Message;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

const GENERATIONS_DIR: &str = "generations";
const CURRENT_MARKER: &str = "current";
const MANIFEST_FILE: &str = "manifest.txt";
static GENERATION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// One complete immutable generation loaded into memory for lock-free response copies.
#[derive(Clone)]
pub struct PublishedGeneration {
    pub id: GenerationId,
    pub manifest: FeedSetManifest,
    pub static_zip: Arc<[u8]>,
    pub trip_updates: Arc<[u8]>,
    pub vehicle_positions: Arc<[u8]>,
    pub alerts: Arc<[u8]>,
}

/// Store-open or recovery failure.
#[derive(Debug)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}

/// Durable publication failure. The store's current pointer is unchanged.
#[derive(Debug)]
pub struct PublishError(pub String);

impl std::fmt::Display for PublishError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PublishError {}

impl From<std::io::Error> for PublishError {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}

/// Recovered immutable generations plus the single currently visible generation.
///
/// `open` ignores temporary and incomplete directories, repairs an invalid marker
/// to the newest complete generation, and finishes recovery before callers can
/// construct the HTTP server. Open `Arc` readers remain valid after later commits.
/// This increment conservatively retains every finalized predecessor, which is
/// stronger than the ten-minute minimum and avoids an incorrect retention clock;
/// bounded cleanup can later use a durable supersession timestamp. The configured
/// output directory is a trust boundary: it must be service-owned, must not be a
/// symlink, and on Unix must not be writable by group or other users.
#[derive(Clone)]
pub struct GenerationStore {
    output_dir: Arc<PathBuf>,
    current: Arc<RwLock<Option<Arc<PublishedGeneration>>>>,
    generations: Arc<RwLock<HashMap<GenerationId, Arc<PublishedGeneration>>>>,
    publish_lock: Arc<Mutex<()>>,
}

impl GenerationStore {
    /// Opens a store and recovers its durable last-good generation, if any.
    pub async fn open(output_dir: &Path) -> Result<Self, StoreError> {
        let output_existed = output_dir.exists();
        fs::create_dir_all(output_dir)?;
        if !output_existed {
            make_directory_private(output_dir)?;
        }
        require_real_directory(output_dir).map_err(StoreError)?;
        let canonical_output = fs::canonicalize(output_dir)?;
        require_trusted_path(&canonical_output).map_err(StoreError)?;
        let output_handle = open_real_directory(&canonical_output).map_err(StoreError)?;
        let generations_dir = canonical_output.join(GENERATIONS_DIR);
        let generations_existed = generations_dir.exists();
        fs::create_dir_all(&generations_dir)?;
        if !generations_existed {
            make_directory_private(&generations_dir)?;
        }
        require_real_directory(&generations_dir).map_err(StoreError)?;
        let generations_handle = open_child_directory(
            &output_handle,
            &canonical_output,
            GENERATIONS_DIR,
            &generations_dir,
        )
        .map_err(StoreError)?;

        let mut recovered = HashMap::new();
        let mut ordered = Vec::new();
        for name in directory_names(&generations_handle, &generations_dir).map_err(StoreError)? {
            let Some(order) = generation_order(&name) else {
                continue;
            };
            if let Ok(generation) = load_generation_at(&generations_handle, &generations_dir, &name)
            {
                let generation = Arc::new(generation);
                ordered.push((order, generation.id.clone()));
                recovered.insert(generation.id.clone(), generation);
            }
        }
        ordered.sort_by_key(|entry| std::cmp::Reverse(entry.0));

        let marker_id = read_regular_at(&output_handle, &canonical_output, CURRENT_MARKER)
            .ok()
            .and_then(|value| String::from_utf8(value).ok())
            .and_then(|value| parse_generation_id(value.trim()));
        let selected = marker_id
            .as_ref()
            .and_then(|id| recovered.get(id).cloned())
            .or_else(|| {
                ordered
                    .first()
                    .and_then(|(_, id)| recovered.get(id).cloned())
            });

        if marker_id.as_ref() != selected.as_ref().map(|generation| &generation.id) {
            if let Some(generation) = selected.as_ref() {
                replace_marker(&canonical_output, &generation.id)?;
            }
        }

        Ok(Self {
            output_dir: Arc::new(canonical_output),
            current: Arc::new(RwLock::new(selected)),
            generations: Arc::new(RwLock::new(recovered)),
            publish_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Returns one immutable snapshot of the current generation.
    pub async fn current(&self) -> Option<Arc<PublishedGeneration>> {
        self.current_sync()
    }

    /// Resolves only generations recovered or committed as complete.
    pub async fn get(&self, id: &GenerationId) -> Option<Arc<PublishedGeneration>> {
        self.generations.read().ok()?.get(id).cloned()
    }

    fn current_sync(&self) -> Option<Arc<PublishedGeneration>> {
        self.current.read().ok()?.clone()
    }

    fn commit(&self, generation: Arc<PublishedGeneration>) -> Result<(), PublishError> {
        self.generations
            .write()
            .map_err(|_| PublishError("generation map lock poisoned".into()))?
            .insert(generation.id.clone(), generation.clone());
        *self
            .current
            .write()
            .map_err(|_| PublishError("current generation lock poisoned".into()))? =
            Some(generation);
        Ok(())
    }
}

/// Persists a validated generation and performs one visibility swap only after
/// artifacts, directory renames, and the current marker are durable.
pub struct GenerationPublisher;

impl GenerationPublisher {
    pub async fn publish(
        output_dir: &Path,
        store: &GenerationStore,
        candidate: ValidatedGeneration,
    ) -> Result<Arc<PublishedGeneration>, PublishError> {
        publish_inner(output_dir, store, candidate, None)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FailurePoint {
    StaticArtifact,
    TripUpdatesArtifact,
    VehiclePositionsArtifact,
    AlertsArtifact,
    ManifestArtifact,
    TemporaryDirectory,
    GenerationRename,
    GenerationsDirectory,
    MarkerArtifact,
    MarkerRename,
    MarkerDirectory,
}

fn publish_inner(
    output_dir: &Path,
    store: &GenerationStore,
    candidate: ValidatedGeneration,
    failure: Option<FailurePoint>,
) -> Result<Arc<PublishedGeneration>, PublishError> {
    let requested_output = fs::canonicalize(output_dir)?;
    if requested_output != store.output_dir.as_path() {
        return Err(PublishError(
            "publisher output directory does not match the opened store".into(),
        ));
    }
    let output_dir = store.output_dir.as_path();
    let _publish_guard = store
        .publish_lock
        .lock()
        .map_err(|_| PublishError("publisher lock poisoned".into()))?;
    let generations_dir = output_dir.join(GENERATIONS_DIR);
    let id = next_generation_id(candidate.generated_at_unix, &generations_dir)?;
    let temporary_dir = generations_dir.join(format!(".{}.tmp", id.0));
    let final_dir = generations_dir.join(&id.0);
    fs::create_dir(&temporary_dir)?;
    make_directory_private(&temporary_dir)?;

    let urls = artifact_urls(&id);
    let manifest = candidate.manifest(id.clone(), urls);
    write_synced(
        &temporary_dir.join("static.zip"),
        &candidate.static_snapshot.zip,
    )?;
    fail_at(failure, FailurePoint::StaticArtifact)?;
    write_synced(
        &temporary_dir.join("trip-updates.pb"),
        &candidate.trip_updates,
    )?;
    fail_at(failure, FailurePoint::TripUpdatesArtifact)?;
    write_synced(
        &temporary_dir.join("vehicle-positions.pb"),
        &candidate.vehicle_positions,
    )?;
    fail_at(failure, FailurePoint::VehiclePositionsArtifact)?;
    write_synced(&temporary_dir.join("alerts.pb"), &candidate.alerts)?;
    fail_at(failure, FailurePoint::AlertsArtifact)?;
    let manifest_bytes = encode_manifest(
        &manifest,
        &candidate.static_snapshot.zip,
        &candidate.trip_updates,
        &candidate.vehicle_positions,
        &candidate.alerts,
    );
    write_synced(&temporary_dir.join(MANIFEST_FILE), &manifest_bytes)?;
    fail_at(failure, FailurePoint::ManifestArtifact)?;
    sync_directory(&temporary_dir)?;
    fail_at(failure, FailurePoint::TemporaryDirectory)?;

    fs::rename(&temporary_dir, &final_dir)?;
    fail_at(failure, FailurePoint::GenerationRename)?;
    sync_directory(&generations_dir)?;
    fail_at(failure, FailurePoint::GenerationsDirectory)?;

    let marker_tmp = marker_temporary(output_dir, &id);
    write_synced(&marker_tmp, format!("{}\n", id.0).as_bytes())?;
    fail_at(failure, FailurePoint::MarkerArtifact)?;
    fs::rename(&marker_tmp, output_dir.join(CURRENT_MARKER))?;
    fail_at(failure, FailurePoint::MarkerRename)?;
    sync_directory(output_dir)?;
    fail_at(failure, FailurePoint::MarkerDirectory)?;

    let generation = Arc::new(load_generation(&final_dir, &id.0).map_err(PublishError)?);
    store.commit(generation.clone())?;
    Ok(generation)
}

fn fail_at(actual: Option<FailurePoint>, expected: FailurePoint) -> Result<(), PublishError> {
    if actual == Some(expected) {
        Err(PublishError(format!(
            "injected publication failure after {expected:?}"
        )))
    } else {
        Ok(())
    }
}

fn next_generation_id(
    generated_at_unix: u64,
    generations_dir: &Path,
) -> Result<GenerationId, PublishError> {
    let nanoseconds = u128::from(generated_at_unix) * 1_000_000_000;
    loop {
        let counter = GENERATION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let id = GenerationId(format!("{nanoseconds}-{counter}"));
        if !generations_dir.join(&id.0).exists()
            && !generations_dir.join(format!(".{}.tmp", id.0)).exists()
        {
            return Ok(id);
        }
    }
}

fn generation_order(value: &str) -> Option<(u128, u64)> {
    let (timestamp, counter) = value.split_once('-')?;
    if timestamp.is_empty()
        || counter.is_empty()
        || timestamp.bytes().any(|byte| !byte.is_ascii_digit())
        || counter.bytes().any(|byte| !byte.is_ascii_digit())
    {
        return None;
    }
    Some((timestamp.parse().ok()?, counter.parse().ok()?))
}

fn parse_generation_id(value: &str) -> Option<GenerationId> {
    value.parse().ok()
}

fn artifact_urls(id: &GenerationId) -> ArtifactUrls {
    let prefix = format!("/v1/generations/{}", id.0);
    ArtifactUrls {
        static_zip: format!("{prefix}/static.zip"),
        trip_updates: format!("{prefix}/trip-updates.pb"),
        vehicle_positions: format!("{prefix}/vehicle-positions.pb"),
        alerts: format!("{prefix}/alerts.pb"),
    }
}

fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn make_directory_private(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

fn replace_marker(output_dir: &Path, id: &GenerationId) -> std::io::Result<()> {
    let temporary = marker_temporary(output_dir, id);
    write_synced(&temporary, format!("{}\n", id.0).as_bytes())?;
    fs::rename(temporary, output_dir.join(CURRENT_MARKER))?;
    sync_directory(output_dir)
}

fn marker_temporary(output_dir: &Path, id: &GenerationId) -> PathBuf {
    output_dir.join(format!(
        ".{CURRENT_MARKER}.{}.{}.{}.tmp",
        id.0,
        std::process::id(),
        GENERATION_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

fn require_real_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        Err(format!(
            "store path must be a real service-owned directory: {}",
            path.display()
        ))
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if metadata.permissions().mode() & 0o022 != 0 {
                return Err(format!(
                    "store directory must not be group/world writable: {}",
                    path.display()
                ));
            }
            if metadata.uid() != rustix::process::geteuid().as_raw() {
                return Err(format!(
                    "store directory must be owned by the service user: {}",
                    path.display()
                ));
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
fn open_real_directory(path: &Path) -> Result<File, String> {
    use rustix::fs::{open, Mode, OFlags};

    let descriptor = open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| error.to_string())?;
    validate_directory_descriptor(&descriptor, path)?;
    Ok(File::from(descriptor))
}

#[cfg(not(unix))]
fn open_real_directory(path: &Path) -> Result<File, String> {
    let directory = File::open(path).map_err(|error| error.to_string())?;
    if !directory
        .metadata()
        .map_err(|error| error.to_string())?
        .is_dir()
    {
        return Err(format!("store path is not a directory: {}", path.display()));
    }
    Ok(directory)
}

#[cfg(unix)]
fn validate_directory_descriptor<Fd: std::os::fd::AsFd>(
    descriptor: Fd,
    path: &Path,
) -> Result<(), String> {
    use rustix::fs::{fstat, FileType};

    let stat = fstat(descriptor).map_err(|error| error.to_string())?;
    if !FileType::from_raw_mode(stat.st_mode).is_dir()
        || stat.st_mode & 0o022 != 0
        || stat.st_uid != rustix::process::geteuid().as_raw()
    {
        return Err(format!(
            "opened directory must be real, private, and service-owned: {}",
            path.display()
        ));
    }
    Ok(())
}

fn require_trusted_path(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let canonical = fs::canonicalize(path).map_err(|error| error.to_string())?;
        let service_uid = rustix::process::geteuid().as_raw();
        for ancestor in canonical.ancestors().skip(1) {
            let metadata = fs::symlink_metadata(ancestor).map_err(|error| error.to_string())?;
            let mode = metadata.permissions().mode();
            if mode & 0o022 != 0 && mode & 0o1000 == 0 {
                return Err(format!(
                    "store ancestor is writable without sticky protection: {}",
                    ancestor.display()
                ));
            }
            if metadata.uid() != 0 && metadata.uid() != service_uid {
                return Err(format!(
                    "store ancestor is not owned by root or the service user: {}",
                    ancestor.display()
                ));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(unix)]
fn open_child_directory(
    parent: &File,
    _parent_path: &Path,
    name: &str,
    child_path: &Path,
) -> Result<File, String> {
    use rustix::fs::{openat, Mode, OFlags};

    let descriptor = openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| error.to_string())?;
    validate_directory_descriptor(&descriptor, child_path)?;
    Ok(File::from(descriptor))
}

#[cfg(not(unix))]
fn open_child_directory(
    _parent: &File,
    parent_path: &Path,
    name: &str,
    _child_path: &Path,
) -> Result<File, String> {
    open_real_directory(&parent_path.join(name))
}

#[cfg(unix)]
fn directory_names(directory: &File, _path: &Path) -> Result<Vec<String>, String> {
    use rustix::fs::Dir;

    let mut names = Vec::new();
    let entries = Dir::read_from(directory).map_err(|error| error.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy();
        if name != "." && name != ".." {
            names.push(name.into_owned());
        }
    }
    Ok(names)
}

#[cfg(not(unix))]
fn directory_names(_directory: &File, path: &Path) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for entry in fs::read_dir(path).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

#[cfg(unix)]
fn read_regular_at(directory: &File, directory_path: &Path, name: &str) -> Result<Vec<u8>, String> {
    use rustix::fs::{fstat, openat, FileType, Mode, OFlags};

    let descriptor = openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| error.to_string())?;
    let stat = fstat(&descriptor).map_err(|error| error.to_string())?;
    if !FileType::from_raw_mode(stat.st_mode).is_file() {
        return Err(format!(
            "generation artifact is not a regular file: {}",
            directory_path.join(name).display()
        ));
    }
    let mut file = File::from(descriptor);
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

#[cfg(not(unix))]
fn read_regular_at(
    _directory: &File,
    directory_path: &Path,
    name: &str,
) -> Result<Vec<u8>, String> {
    let path = directory_path.join(name);
    let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(format!(
            "generation artifact is not a regular file: {}",
            path.display()
        ));
    }
    fs::read(path).map_err(|error| error.to_string())
}

fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn hex_encode(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hex_decode(value: &str) -> Result<String, String> {
    if !value.len().is_multiple_of(2) || value.bytes().any(|byte| !byte.is_ascii_hexdigit()) {
        return Err("invalid manifest string encoding".into());
    }
    let bytes = (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "invalid manifest string encoding")?;
    String::from_utf8(bytes).map_err(|_| "manifest string is not UTF-8".into())
}

fn encode_manifest(
    manifest: &FeedSetManifest,
    static_zip: &[u8],
    trip_updates: &[u8],
    vehicle_positions: &[u8],
    alerts: &[u8],
) -> Vec<u8> {
    format!(
        "amtrak-generation-v1\nid={}\ngenerated={}\nstatic={}\nsource={}\ncounts={},{},{}\nchecksums={:016x},{:016x},{:016x},{:016x}\n",
        manifest.generation_id.0,
        manifest.generated_at_unix,
        hex_encode(&manifest.static_version),
        hex_encode(&manifest.source),
        manifest.entity_counts.trip_updates,
        manifest.entity_counts.vehicle_positions,
        manifest.entity_counts.alerts,
        checksum(static_zip),
        checksum(trip_updates),
        checksum(vehicle_positions),
        checksum(alerts),
    )
    .into_bytes()
}

fn load_generation(directory: &Path, expected_id: &str) -> Result<PublishedGeneration, String> {
    require_real_directory(directory)?;
    let directory_handle = open_real_directory(directory)?;
    load_generation_from_handle(&directory_handle, directory, expected_id)
}

fn load_generation_at(
    parent: &File,
    parent_path: &Path,
    expected_id: &str,
) -> Result<PublishedGeneration, String> {
    let directory = parent_path.join(expected_id);
    let directory_handle = open_child_directory(parent, parent_path, expected_id, &directory)?;
    load_generation_from_handle(&directory_handle, &directory, expected_id)
}

fn load_generation_from_handle(
    directory_handle: &File,
    directory: &Path,
    expected_id: &str,
) -> Result<PublishedGeneration, String> {
    let manifest_text =
        String::from_utf8(read_regular_at(directory_handle, directory, MANIFEST_FILE)?)
            .map_err(|_| "generation manifest is not UTF-8")?;
    let lines: Vec<_> = manifest_text.lines().collect();
    if lines.len() != 7 || lines[0] != "amtrak-generation-v1" {
        return Err("invalid generation manifest shape".into());
    }
    let id_value = manifest_field(lines[1], "id=")?;
    let id = parse_generation_id(id_value).ok_or("invalid generation id")?;
    if id.0 != expected_id {
        return Err("generation directory and manifest ID differ".into());
    }
    let generated_at_unix = manifest_field(lines[2], "generated=")?
        .parse::<u64>()
        .map_err(|_| "invalid generation timestamp")?;
    let static_version = hex_decode(manifest_field(lines[3], "static=")?)?;
    let source = hex_decode(manifest_field(lines[4], "source=")?)?;
    let counts = parse_numbers::<usize, 3>(manifest_field(lines[5], "counts=")?)?;
    let expected_checksums = parse_hex_numbers::<4>(manifest_field(lines[6], "checksums=")?)?;

    let static_zip = read_arc_at(directory_handle, directory, "static.zip")?;
    let trip_updates = read_arc_at(directory_handle, directory, "trip-updates.pb")?;
    let vehicle_positions = read_arc_at(directory_handle, directory, "vehicle-positions.pb")?;
    let alerts = read_arc_at(directory_handle, directory, "alerts.pb")?;
    let actual_checksums = [
        checksum(&static_zip),
        checksum(&trip_updates),
        checksum(&vehicle_positions),
        checksum(&alerts),
    ];
    if expected_checksums != actual_checksums || static_zip.is_empty() {
        return Err("artifact checksum mismatch or empty static feed".into());
    }

    for (bytes, expected_count) in [
        (&trip_updates, counts[0]),
        (&vehicle_positions, counts[1]),
        (&alerts, counts[2]),
    ] {
        let message = FeedMessage::decode(bytes.as_ref()).map_err(|error| error.to_string())?;
        if message.entity.len() != expected_count
            || message.header.timestamp != Some(generated_at_unix)
            || message.header.feed_version.as_deref() != Some(static_version.as_str())
        {
            return Err("recovered protobuf does not match its manifest".into());
        }
    }

    let entity_counts = EntityCounts {
        trip_updates: counts[0],
        vehicle_positions: counts[1],
        alerts: counts[2],
    };
    let manifest = FeedSetManifest {
        generation_id: id.clone(),
        generated_at_unix,
        static_version,
        source,
        entity_counts,
        urls: artifact_urls(&id),
    };
    Ok(PublishedGeneration {
        id,
        manifest,
        static_zip,
        trip_updates,
        vehicle_positions,
        alerts,
    })
}

fn manifest_field<'a>(line: &'a str, prefix: &str) -> Result<&'a str, String> {
    line.strip_prefix(prefix)
        .ok_or_else(|| format!("missing manifest field {prefix}"))
}

fn parse_numbers<T, const N: usize>(value: &str) -> Result<[T; N], String>
where
    T: std::str::FromStr,
{
    value
        .split(',')
        .map(|part| part.parse().map_err(|_| "invalid manifest number".into()))
        .collect::<Result<Vec<T>, String>>()?
        .try_into()
        .map_err(|_| "wrong manifest number count".into())
}

fn parse_hex_numbers<const N: usize>(value: &str) -> Result<[u64; N], String> {
    value
        .split(',')
        .map(|part| u64::from_str_radix(part, 16).map_err(|_| "invalid checksum".into()))
        .collect::<Result<Vec<_>, String>>()?
        .try_into()
        .map_err(|_| "wrong checksum count".into())
}

fn read_arc_at(directory: &File, directory_path: &Path, name: &str) -> Result<Arc<[u8]>, String> {
    read_regular_at(directory, directory_path, name).map(Arc::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::{StaticSnapshot, ValidatedGeneration};
    use gtfs_realtime::{feed_header, FeedHeader};
    use gtfs_structures::Gtfs;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, AtomicU64};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "amtrak-writer-{label}-{}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn candidate(generated_at_unix: u64, marker: u8) -> ValidatedGeneration {
        let feed = FeedMessage {
            header: FeedHeader {
                gtfs_realtime_version: "2.0".into(),
                incrementality: Some(feed_header::Incrementality::FullDataset as i32),
                timestamp: Some(generated_at_unix),
                feed_version: Some("STATIC-V1".into()),
            },
            entity: Vec::new(),
        }
        .encode_to_vec();
        ValidatedGeneration {
            static_snapshot: Arc::new(StaticSnapshot {
                version: "STATIC-V1".into(),
                parsed: Arc::new(Gtfs::default()),
                zip: Arc::from(vec![b'z', marker]),
            }),
            generated_at_unix,
            source_name: "fixture",
            trip_updates: Arc::from(feed.clone()),
            vehicle_positions: Arc::from(feed.clone()),
            alerts: Arc::from(feed),
            entity_counts: EntityCounts::default(),
        }
    }

    #[tokio::test]
    async fn publishes_recovers_and_keeps_young_predecessors() {
        let dir = test_dir("recover");
        let store = GenerationStore::open(&dir).await.unwrap();
        let first = GenerationPublisher::publish(&dir, &store, candidate(100, 1))
            .await
            .unwrap();
        let second = GenerationPublisher::publish(&dir, &store, candidate(100, 2))
            .await
            .unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(store.current().await.unwrap().id, second.id);
        assert!(store.get(&first.id).await.is_some());
        assert_eq!(second.manifest.generated_at_unix, 100);

        let reopened = GenerationStore::open(&dir).await.unwrap();
        assert_eq!(reopened.current().await.unwrap().id, second.id);
        assert!(reopened.get(&first.id).await.is_some());
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn recovery_repairs_bad_marker_and_ignores_partial_state() {
        let dir = test_dir("repair");
        let store = GenerationStore::open(&dir).await.unwrap();
        let published = GenerationPublisher::publish(&dir, &store, candidate(200, 1))
            .await
            .unwrap();
        fs::write(dir.join(CURRENT_MARKER), "bad/marker\n").unwrap();
        let partial = dir.join(GENERATIONS_DIR).join(".999-1.tmp");
        fs::create_dir(&partial).unwrap();
        fs::write(partial.join("static.zip"), b"partial").unwrap();

        let reopened = GenerationStore::open(&dir).await.unwrap();
        assert_eq!(reopened.current().await.unwrap().id, published.id);
        assert_eq!(
            fs::read_to_string(dir.join(CURRENT_MARKER)).unwrap().trim(),
            published.id.0
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn every_failure_boundary_preserves_in_memory_current() {
        let points = [
            FailurePoint::StaticArtifact,
            FailurePoint::TripUpdatesArtifact,
            FailurePoint::VehiclePositionsArtifact,
            FailurePoint::AlertsArtifact,
            FailurePoint::ManifestArtifact,
            FailurePoint::TemporaryDirectory,
            FailurePoint::GenerationRename,
            FailurePoint::GenerationsDirectory,
            FailurePoint::MarkerArtifact,
            FailurePoint::MarkerRename,
            FailurePoint::MarkerDirectory,
        ];
        for (index, point) in points.into_iter().enumerate() {
            let dir = test_dir("fault");
            let store = GenerationStore::open(&dir).await.unwrap();
            let first = GenerationPublisher::publish(&dir, &store, candidate(300, 1))
                .await
                .unwrap();
            assert!(publish_inner(&dir, &store, candidate(301, index as u8), Some(point)).is_err());
            assert_eq!(store.current().await.unwrap().id, first.id);

            let reopened = GenerationStore::open(&dir).await.unwrap();
            let recovered = reopened.current().await.unwrap();
            assert!(recovered.id == first.id || recovered.manifest.generated_at_unix == 301);
            FeedMessage::decode(recovered.trip_updates.as_ref()).unwrap();
            FeedMessage::decode(recovered.vehicle_positions.as_ref()).unwrap();
            FeedMessage::decode(recovered.alerts.as_ref()).unwrap();
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_readers_observe_only_complete_old_or_new_generation() {
        let dir = test_dir("race");
        let store = GenerationStore::open(&dir).await.unwrap();
        let first = GenerationPublisher::publish(&dir, &store, candidate(400, 1))
            .await
            .unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(HashSet::new()));
        let reader_store = store.clone();
        let reader_stop = stop.clone();
        let reader_seen = seen.clone();
        let reader = std::thread::spawn(move || {
            while !reader_stop.load(Ordering::Relaxed) {
                let generation = reader_store.current_sync().unwrap();
                assert_eq!(
                    generation.manifest.generation_id, generation.id,
                    "manifest and artifact snapshot must share one generation"
                );
                FeedMessage::decode(generation.trip_updates.as_ref()).unwrap();
                reader_seen.lock().unwrap().insert(generation.id.clone());
            }
        });
        while seen.lock().unwrap().is_empty() {
            std::thread::yield_now();
        }
        let second = GenerationPublisher::publish(&dir, &store, candidate(401, 2))
            .await
            .unwrap();
        stop.store(true, Ordering::Relaxed);
        reader.join().unwrap();
        {
            let observed = seen.lock().unwrap();
            assert!(observed
                .iter()
                .all(|id| id == &first.id || id == &second.id));
        }
        assert_eq!(store.current().await.unwrap().id, second.id);
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn corrupt_finalized_generation_is_not_recovered() {
        let dir = test_dir("corrupt");
        let store = GenerationStore::open(&dir).await.unwrap();
        let last_good = GenerationPublisher::publish(&dir, &store, candidate(500, 1))
            .await
            .unwrap();
        let corrupt = GenerationPublisher::publish(&dir, &store, candidate(501, 2))
            .await
            .unwrap();
        fs::write(
            dir.join(GENERATIONS_DIR)
                .join(&corrupt.id.0)
                .join("alerts.pb"),
            b"corrupt",
        )
        .unwrap();
        let reopened = GenerationStore::open(&dir).await.unwrap();
        assert_eq!(reopened.current().await.unwrap().id, last_good.id);
        assert_eq!(
            fs::read_to_string(dir.join(CURRENT_MARKER)).unwrap().trim(),
            last_good.id.0
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn recovery_rejects_symlinked_artifacts() {
        use std::os::unix::fs::symlink;

        let dir = test_dir("symlink-recovery");
        let store = GenerationStore::open(&dir).await.unwrap();
        let last_good = GenerationPublisher::publish(&dir, &store, candidate(600, 1))
            .await
            .unwrap();
        let suspect = GenerationPublisher::publish(&dir, &store, candidate(601, 2))
            .await
            .unwrap();
        let artifact = dir
            .join(GENERATIONS_DIR)
            .join(&suspect.id.0)
            .join("alerts.pb");
        let external = dir.join("external-alerts.pb");
        fs::copy(&artifact, &external).unwrap();
        fs::remove_file(&artifact).unwrap();
        symlink(&external, &artifact).unwrap();

        let reopened = GenerationStore::open(&dir).await.unwrap();
        assert_eq!(reopened.current().await.unwrap().id, last_good.id);
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn publication_does_not_follow_preexisting_marker_temp_symlink() {
        use std::os::unix::fs::symlink;

        let dir = test_dir("symlink-marker");
        let store = GenerationStore::open(&dir).await.unwrap();
        let external = dir.join("external-marker-target");
        fs::write(&external, b"unchanged").unwrap();
        symlink(&external, dir.join(".current.tmp")).unwrap();
        assert!(write_synced(&dir.join(".current.tmp"), b"attacker-controlled").is_err());

        GenerationPublisher::publish(&dir, &store, candidate(700, 1))
            .await
            .unwrap();
        assert_eq!(fs::read(&external).unwrap(), b"unchanged");
        assert!(fs::symlink_metadata(dir.join(".current.tmp"))
            .unwrap()
            .file_type()
            .is_symlink());
        fs::remove_dir_all(dir).unwrap();
    }
}
