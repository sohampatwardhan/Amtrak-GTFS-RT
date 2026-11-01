//! Platform add-on laid over Amtrak's official static GTFS.
//!
//! `official-static.zip` is the download, byte for byte. `platform-addon.zip` holds only the
//! files that differ after the dangling-stop repair and platform stops, plus `addon.txt`.
//! `static.zip` stays the combined feed the existing URL already serves. A consumer rebuilds
//! that combined feed by unzipping the official zip and replacing each file named in
//! `addon.txt`.

use crate::static_augment::{fail, read_entry, AugmentError};
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

/// Schema token written in `addon.txt` and used as the version when the overlay is empty.
pub const ADDON_SCHEMA: &str = "amtrak-platform-addon/v1";

const ADDON_NOTE: &str = "addon.txt";

/// Overlay zip and its version for `combined` relative to `official`.
///
/// # Arguments
///
/// * `official` - Untouched upstream GTFS zip.
/// * `combined` - Zip served at `static.zip` after repair and platform stops.
///
/// # Returns
///
/// The add-on zip and a version of `amtrak-platform-addon/v1`, with a digest when a file changed.
/// An unreadable zip pair yields an empty overlay rather than a different schedule.
pub fn build_addon(official: &[u8], combined: &[u8]) -> (Vec<u8>, String) {
    let changed = changed_files(official, combined).unwrap_or_default();
    let version = addon_version(&changed);
    let zip = write_addon(&version, &changed).unwrap_or_else(|_| bare_zip());
    (zip, version)
}

/// File names the add-on says a consumer should copy onto the official zip.
///
/// # Arguments
///
/// * `addon` - Add-on zip from [`build_addon`].
///
/// # Returns
///
/// Names from the `files` line, in listed order. An add-on without that line yields nothing.
pub fn listed_files(addon: &[u8]) -> Vec<String> {
    let Ok(note) = read_entry(addon, ADDON_NOTE) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&note);
    text.lines()
        .find_map(|line| line.strip_prefix("files="))
        .unwrap_or("")
        .split(',')
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

fn addon_version(changed: &[(String, Vec<u8>)]) -> String {
    if changed.is_empty() {
        return ADDON_SCHEMA.to_string();
    }
    let mut hasher = Sha256::new();
    for (name, bytes) in changed {
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(bytes);
    }
    let digest = format!("{:x}", hasher.finalize());
    format!("{ADDON_SCHEMA}.{}", &digest[..12])
}

fn changed_files(official: &[u8], combined: &[u8]) -> Result<Vec<(String, Vec<u8>)>, AugmentError> {
    let mut official_archive =
        ZipArchive::new(Cursor::new(official)).map_err(|error| fail("official", error))?;
    let mut combined_archive =
        ZipArchive::new(Cursor::new(combined)).map_err(|error| fail("combined", error))?;
    let mut names = Vec::new();
    for index in 0..combined_archive.len() {
        let name = combined_archive
            .by_index(index)
            .map_err(|error| fail("combined entry", error))?
            .name()
            .to_string();
        if name.ends_with('/') {
            continue;
        }
        names.push(name);
    }
    names.sort();
    let mut changed = Vec::new();
    for name in names {
        let mut combined_bytes = Vec::new();
        combined_archive
            .by_name(&name)
            .map_err(|error| fail(&name, error))?
            .read_to_end(&mut combined_bytes)
            .map_err(|error| fail(&name, error))?;
        let official_bytes = official_archive.by_name(&name).ok().and_then(|mut entry| {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).ok()?;
            Some(bytes)
        });
        if official_bytes.as_deref() != Some(combined_bytes.as_slice()) {
            changed.push((name, combined_bytes));
        }
    }
    Ok(changed)
}

fn write_addon(version: &str, changed: &[(String, Vec<u8>)]) -> Result<Vec<u8>, AugmentError> {
    let files = changed
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let note = format!("schema={ADDON_SCHEMA}\nversion={version}\nfiles={files}\n");
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(DateTime::default());
    writer
        .start_file(ADDON_NOTE, options)
        .map_err(|error| fail(ADDON_NOTE, error))?;
    writer
        .write_all(note.as_bytes())
        .map_err(|error| fail(ADDON_NOTE, error))?;
    for (name, bytes) in changed {
        writer
            .start_file(name, options)
            .map_err(|error| fail(name, error))?;
        writer.write_all(bytes).map_err(|error| fail(name, error))?;
    }
    Ok(writer
        .finish()
        .map_err(|error| fail("finish", error))?
        .into_inner())
}

fn bare_zip() -> Vec<u8> {
    write_addon(ADDON_SCHEMA, &[]).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::static_augment::{augment_static, PlatformTable};
    use crate::static_repair::repair_static;
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    fn mardi_gras_zip(stop_times: &str) -> Vec<u8> {
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        let files = [
            ("agency.txt", "agency_id,agency_name,agency_url,agency_timezone\n51,Amtrak,https://www.amtrak.com,America/New_York\n"),
            ("stops.txt", "stop_id,stop_name\nBIX,Biloxi\nNHV,New Haven\nPAG,Pascagoula\n"),
            ("routes.txt", "route_id,route_type\n42996,2\n"),
            ("trips.txt", "route_id,service_id,trip_id\n42996,svc,312328\n"),
            ("stop_times.txt", stop_times),
            ("calendar_dates.txt", "service_id,date,exception_type\nsvc,20261019,1\n"),
            ("feed_info.txt", "feed_publisher_name,feed_version\nAmtrak,20261018\n"),
        ];
        for (name, contents) in files {
            archive.start_file(name, options).unwrap();
            archive.write_all(contents.as_bytes()).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }

    const DANGLING: &str =
        "trip_id,stop_id,stop_sequence\n312328,BIX,4\n312328,OSM,5\n312328,PAG,6\n";

    #[test]
    fn an_unchanged_zip_has_an_empty_overlay() {
        let official = mardi_gras_zip("trip_id,stop_id,stop_sequence\n312328,BIX,4\n");
        let (addon, version) = build_addon(&official, &official);
        assert_eq!(version, ADDON_SCHEMA);
        assert!(listed_files(&addon).is_empty());
        assert_eq!(read_entry(&addon, ADDON_NOTE).is_ok(), true);
    }

    #[test]
    fn repair_and_platforms_overlay_back_onto_the_official_zip() {
        let official = mardi_gras_zip(DANGLING);
        let repaired = repair_static(&official).unwrap();
        let combined = augment_static(&repaired, &PlatformTable::parse("NHV=1").unwrap()).unwrap();
        let (addon, version) = build_addon(&official, &combined);
        assert!(version.starts_with(&format!("{ADDON_SCHEMA}.")));
        let files = listed_files(&addon);
        assert_eq!(files, ["stop_times.txt", "stops.txt"]);
        for name in &files {
            assert_eq!(
                read_entry(&addon, name).unwrap(),
                read_entry(&combined, name).unwrap()
            );
            assert_ne!(
                read_entry(&official, name).unwrap(),
                read_entry(&combined, name).unwrap()
            );
        }
        assert_eq!(
            read_entry(&official, "trips.txt").unwrap(),
            read_entry(&combined, "trips.txt").unwrap()
        );
        assert!(
            !String::from_utf8(read_entry(&addon, "stop_times.txt").unwrap())
                .unwrap()
                .contains("OSM")
        );
        assert!(
            String::from_utf8(read_entry(&official, "stop_times.txt").unwrap())
                .unwrap()
                .contains("OSM")
        );
        assert!(String::from_utf8(read_entry(&addon, "stops.txt").unwrap())
            .unwrap()
            .contains("NHV:track:1"));
    }
}
