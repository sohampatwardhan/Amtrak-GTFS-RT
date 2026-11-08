//! Fleet snapshot shared by the three feed roles.
//!
//! VehiclePositions writes one snapshot per poll. TripUpdates and Alerts write
//! their protobufs beside it. Nothing here performs network I/O.

#![deny(missing_docs)]

use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Metadata stored next to one fleet snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FleetMeta {
    /// `<seconds>-<counter>` identifier for this poll.
    pub id: String,
    /// Unix seconds stamped into every realtime header.
    pub generated_at_unix: u64,
    /// Static `feed_version`, including a tracks suffix when the ZIP has one.
    pub static_version: String,
    /// Version of the platform add-on stored beside this snapshot.
    pub addon_version: String,
}

/// Writes one fleet snapshot atomically and points `fleet/CURRENT` at it.
///
/// # Arguments
///
/// * `output_dir` - Service data directory.
/// * `meta` - Identity of this poll.
/// * `trip_updates` - Trip updates before track stamping.
/// * `vehicle_positions` - Vehicle positions for this poll.
/// * `alerts` - Alerts from the fleet poll, before notice merging.
/// * `trains` - `trains.json` from [`crate::fleet_trains::train_list`], read by the track poller.
/// * `static_zip` - Combined static GTFS the poll was matched against.
/// * `official_zip` - Untouched official GTFS download.
/// * `addon_zip` - Overlay that rebuilds the combined zip.
///
/// # Errors
///
/// Returns an I/O error when the directory cannot be created or renamed.
/// A failure leaves `fleet/CURRENT` unchanged.
///
/// # Examples
///
/// ```
/// use amtrak_gtfs_rt_service::cohort::{write_fleet, FleetMeta};
///
/// let dir = std::env::temp_dir().join(format!("fleet-doc-{}", std::process::id()));
/// let _ = std::fs::remove_dir_all(&dir);
/// write_fleet(
///     &dir,
///     &FleetMeta {
///         id: "10-1".into(),
///         generated_at_unix: 10,
///         static_version: "v".into(),
///         addon_version: "amtrak-platform-addon/v1".into(),
///     },
///     b"tu", b"vp", b"al", b"{}", b"zip", b"official", b"addon",
/// ).unwrap();
/// assert_eq!(std::fs::read(dir.join("fleet/CURRENT")).unwrap(), b"10-1\n");
/// let _ = std::fs::remove_dir_all(&dir);
/// ```
pub fn write_fleet(
    output_dir: &Path,
    meta: &FleetMeta,
    trip_updates: &[u8],
    vehicle_positions: &[u8],
    alerts: &[u8],
    trains: &[u8],
    static_zip: &[u8],
    official_zip: &[u8],
    addon_zip: &[u8],
) -> std::io::Result<()> {
    let fleet = output_dir.join("fleet");
    fs::create_dir_all(&fleet)?;
    let final_dir = fleet.join(&meta.id);
    let temporary = fleet.join(format!(".{}", meta.id));
    let _ = fs::remove_dir_all(&temporary);
    fs::create_dir(&temporary)?;
    write_file(&temporary.join("meta.json"), meta_bytes(meta).as_bytes())?;
    write_file(&temporary.join("trip-updates.pb"), trip_updates)?;
    write_file(&temporary.join("vehicle-positions.pb"), vehicle_positions)?;
    write_file(&temporary.join("alerts.pb"), alerts)?;
    write_file(&temporary.join("trains.json"), trains)?;
    write_file(&temporary.join("static.zip"), static_zip)?;
    write_file(&temporary.join("official-static.zip"), official_zip)?;
    write_file(&temporary.join("platform-addon.zip"), addon_zip)?;
    let _ = fs::remove_dir_all(&final_dir);
    fs::rename(&temporary, &final_dir)?;
    write_file(&fleet.join("CURRENT"), format!("{}\n", meta.id).as_bytes())?;
    Ok(())
}

/// The id in `fleet/CURRENT`, if a snapshot has been written.
///
/// # Arguments
///
/// * `output_dir` - Service data directory.
///
/// # Returns
///
/// The current fleet id, or `None` when the marker is missing.
pub fn current_fleet_id(output_dir: &Path) -> Option<String> {
    let text = fs::read_to_string(output_dir.join("fleet/CURRENT")).ok()?;
    let id = text.trim();
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

/// Reads the metadata for one fleet id.
///
/// # Arguments
///
/// * `output_dir` - Service data directory.
/// * `id` - Fleet id from [`current_fleet_id`].
///
/// # Errors
///
/// Returns an error when the file is missing or not the expected object.
pub fn read_meta(output_dir: &Path, id: &str) -> Result<FleetMeta, String> {
    let path = fleet_dir(output_dir, id).join("meta.json");
    let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let value: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let object = value.as_object().ok_or("fleet meta is not an object")?;
    Ok(FleetMeta {
        id: object
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        generated_at_unix: object
            .get("generated_at_unix")
            .and_then(Value::as_u64)
            .ok_or("generated_at_unix missing")?,
        static_version: object
            .get("static_version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        addon_version: object
            .get("addon_version")
            .and_then(Value::as_str)
            .unwrap_or("amtrak-platform-addon/v1")
            .to_string(),
    })
}

/// Writes one contributed protobuf for a fleet id.
///
/// # Arguments
///
/// * `output_dir` - Service data directory.
/// * `id` - Fleet id the bytes belong to.
/// * `name` - `trip-updates.pb` or `alerts.pb`.
/// * `bytes` - Protobuf body.
///
/// # Errors
///
/// Returns an I/O error when the contribution cannot be written.
pub fn write_contribution(
    output_dir: &Path,
    id: &str,
    name: &str,
    bytes: &[u8],
) -> std::io::Result<()> {
    let dir = output_dir.join("contrib").join(id);
    fs::create_dir_all(&dir)?;
    let temporary = dir.join(format!(".{name}"));
    write_file(&temporary, bytes)?;
    fs::rename(&temporary, dir.join(name))?;
    Ok(())
}

/// Whether both sibling contributions exist for `id`.
///
/// # Arguments
///
/// * `output_dir` - Service data directory.
/// * `id` - Fleet id.
///
/// # Returns
///
/// `true` when trip updates and alerts have both been written.
pub fn contributions_ready(output_dir: &Path, id: &str) -> bool {
    let dir = output_dir.join("contrib").join(id);
    dir.join("trip-updates.pb").is_file() && dir.join("alerts.pb").is_file()
}

/// Path of one file inside a fleet snapshot.
///
/// # Arguments
///
/// * `output_dir` - Service data directory.
/// * `id` - Fleet id.
/// * `name` - File name inside the snapshot.
///
/// # Returns
///
/// The path. The file is not required to exist.
pub fn fleet_file(output_dir: &Path, id: &str, name: &str) -> PathBuf {
    fleet_dir(output_dir, id).join(name)
}

/// Path of one contributed file.
///
/// # Arguments
///
/// * `output_dir` - Service data directory.
/// * `id` - Fleet id.
/// * `name` - `trip-updates.pb` or `alerts.pb`.
///
/// # Returns
///
/// The path. The file is not required to exist.
pub fn contribution_file(output_dir: &Path, id: &str, name: &str) -> PathBuf {
    output_dir.join("contrib").join(id).join(name)
}

fn fleet_dir(output_dir: &Path, id: &str) -> PathBuf {
    output_dir.join("fleet").join(id)
}

fn meta_bytes(meta: &FleetMeta) -> String {
    json!({
        "id": meta.id,
        "generated_at_unix": meta.generated_at_unix,
        "static_version": meta.static_version,
        "addon_version": meta.addon_version,
    })
    .to_string()
}

fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_is_current_only_after_the_rename_and_contribs_are_separate() {
        let dir = std::env::temp_dir().join(format!(
            "cohort-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let meta = FleetMeta {
            id: "15-1".into(),
            generated_at_unix: 15,
            static_version: "2026".into(),
            addon_version: "amtrak-platform-addon/v1".into(),
        };
        write_fleet(
            &dir,
            &meta,
            b"tu",
            b"vp",
            b"al",
            b"{}",
            b"zip",
            b"official",
            b"addon",
        )
        .unwrap();
        assert_eq!(current_fleet_id(&dir).as_deref(), Some("15-1"));
        assert_eq!(read_meta(&dir, "15-1").unwrap(), meta);
        assert!(!contributions_ready(&dir, "15-1"));
        write_contribution(&dir, "15-1", "trip-updates.pb", b"stamped").unwrap();
        assert!(!contributions_ready(&dir, "15-1"));
        write_contribution(&dir, "15-1", "alerts.pb", b"merged").unwrap();
        assert!(contributions_ready(&dir, "15-1"));
        let _ = fs::remove_dir_all(&dir);
    }
}
