//! Tells whoever started the service where its feed is.
//!
//! At startup the service prints its endpoints and which optional sources are on. When the first
//! generation is published it prints that generation's file URLs once, so a reader can fetch a
//! protobuf without composing the manifest lookup by hand. Only local endpoints and source names
//! are printed: no upstream URL, credential, or response text.

use crate::config::{is_worker_url, AccessPolicy, AdvisoryConfig, TrackConfig};
use crate::orchestrator::{GenerationCommitter, ValidatedGeneration};
use crate::writer::{PublishError, PublishedGeneration};
use async_trait::async_trait;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Base URL a client on this machine uses: `public_url` when set (a published container port),
/// otherwise the bind address with a wildcard host replaced by loopback.
pub fn base_url(bind: SocketAddr, public_url: Option<&str>) -> String {
    if let Some(url) = public_url {
        return url.to_string();
    }
    let host = match bind.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    format!("http://{}", SocketAddr::new(host, bind.port()))
}

/// The startup banner: endpoints, who may read them, and which optional sources are on.
///
/// Each source line says whether it is on and, when off, which flag or empty URL turned it off.
/// Source URLs are never printed; a source reading the built-in browser worker says so.
pub fn startup_banner(
    base: &str,
    bind: SocketAddr,
    access: &AccessPolicy,
    advisories: Option<&AdvisoryConfig>,
    tracks: Option<&TrackConfig>,
    filter_capital_corridor: bool,
) -> String {
    let mut lines = vec![
        format!("amtrak-gtfs-rt listening on {bind}"),
        format!("  Feed set (start here)  {base}/v1/feed-set.json"),
        format!(
            "  Generation files       {base}/v1/generations/{{id}}/\
             {{static.zip,official-static.zip,platform-addon.zip,trip-updates.pb,vehicle-positions.pb,alerts.pb}}"
        ),
        format!("  Readiness              {base}/readyz"),
        format!("  Liveness               {base}/livez"),
        format!("  Access                 {}", access.describe()),
        format!(
            "  Advisories             {}",
            match advisories {
                Some(notices) => on_from(&notices.url, "AMTRAK_ADVISORIES_URL"),
                None => "off".to_string(),
            }
        ),
        format!(
            "  Capital Corridor       {}",
            if filter_capital_corridor {
                "filtered out"
            } else {
                "kept"
            }
        ),
    ];
    match tracks {
        None => lines.push("  Tracks                 off".to_string()),
        Some(tracks) => {
            lines.push("  Tracks                 on".to_string());
            lines.extend(track_sources(tracks));
        }
    }
    lines.join("\n")
}

/// "on" plus where the source reads from, naming the override variable instead of its URL.
fn on_from(url: &str, override_var: &str) -> String {
    if is_worker_url(url) {
        "on (built-in browser worker)".to_string()
    } else {
        format!("on ({override_var})")
    }
}

fn track_sources(tracks: &TrackConfig) -> Vec<String> {
    let amtrak = match (&tracks.amtrak_url, tracks.amtrak_enabled) {
        (Some(url), _) => on_from(url, "AMTRAK_TRACKS_AMTRAK_URL"),
        (None, false) => "off: AMTRAK_TRACKS_AMTRAK=off".to_string(),
        (None, true) => "off: AMTRAK_TRACKS_AMTRAK_URL is empty".to_string(),
    };
    let raildata = match (
        tracks.njt_stations.is_empty(),
        tracks.raildata_credentials().is_some(),
    ) {
        (true, _) => "off: no stations".to_string(),
        (false, false) => "off: no NJT_RAILDATA credentials".to_string(),
        (false, true) => format!("on ({} stations)", tracks.njt_stations.len()),
    };
    let hartford = match (&tracks.hartford_url, tracks.hartford_enabled) {
        (Some(_), _) => format!("on ({})", tracks.hartford_stop_id),
        (None, false) => "off: AMTRAK_TRACKS_HARTFORD=off".to_string(),
        (None, true) => "off: AMTRAK_TRACKS_HARTFORD_URL is empty".to_string(),
    };
    vec![
        format!("    Amtrak status        {amtrak}"),
        format!("    NJ Transit RailData  {raildata}"),
        format!("    Hartford Line board  {hartford}"),
    ]
}

/// The file URLs of one published generation.
pub fn generation_urls(base: &str, generation: &PublishedGeneration) -> String {
    let urls = &generation.manifest.urls;
    [
        format!(
            "First generation {} published:",
            generation.manifest.generation_id.0
        ),
        format!("  Vehicle positions      {base}{}", urls.vehicle_positions),
        format!("  Trip updates           {base}{}", urls.trip_updates),
        format!("  Alerts                 {base}{}", urls.alerts),
        format!("  Static schedule        {base}{}", urls.static_zip),
        format!(
            "  Official static GTFS   {base}{}",
            urls.official_static_zip
        ),
        format!("  Platform add-on        {base}{}", urls.platform_addon_zip),
        "  Generation URLs change on every refresh; read the feed set for the current ones."
            .to_string(),
    ]
    .join("\n")
}

/// Prints the first published generation's file URLs, then stays quiet.
pub struct AnnounceFirstGeneration<C> {
    inner: C,
    base: String,
    announced: AtomicBool,
}

impl<C> AnnounceFirstGeneration<C> {
    pub fn new(inner: C, base: String) -> Self {
        Self {
            inner,
            base,
            announced: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl<C: GenerationCommitter> GenerationCommitter for AnnounceFirstGeneration<C> {
    async fn publish(
        &self,
        candidate: ValidatedGeneration,
    ) -> Result<Arc<PublishedGeneration>, PublishError> {
        let published = self.inner.publish(candidate).await?;
        if !self.announced.swap(true, Ordering::Relaxed) {
            println!("{}", generation_urls(&self.base, &published));
        }
        Ok(published)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn tracks(pairs: &[(&str, &str)]) -> TrackConfig {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        TrackConfig::from_map(|key| {
            pairs
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        })
        .unwrap()
    }

    #[test]
    fn wildcard_binds_are_shown_on_loopback() {
        assert_eq!(
            base_url("0.0.0.0:8080".parse().unwrap(), None),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            base_url("[::]:8080".parse().unwrap(), None),
            "http://[::1]:8080"
        );
        assert_eq!(
            base_url("127.0.0.1:9000".parse().unwrap(), None),
            "http://127.0.0.1:9000"
        );
        assert_eq!(
            base_url(
                "0.0.0.0:8080".parse().unwrap(),
                Some("http://127.0.0.1:8090")
            ),
            "http://127.0.0.1:8090"
        );
    }

    #[test]
    fn the_banner_lists_endpoints_and_sources() {
        let bind = "127.0.0.1:8080".parse().unwrap();
        let access = AccessPolicy::from_allowed_ips_for_test(BTreeSet::new());
        let off = startup_banner("http://127.0.0.1:8080", bind, &access, None, None, false);
        assert!(off.contains("http://127.0.0.1:8080/v1/feed-set.json"));
        assert!(off.contains(
            "http://127.0.0.1:8080/v1/generations/{id}/\
             {static.zip,official-static.zip,platform-addon.zip,trip-updates.pb,vehicle-positions.pb,alerts.pb}"
        ));
        assert!(off.contains("http://127.0.0.1:8080/readyz"));
        assert!(off.contains("loopback clients only"));
        assert!(off.contains("Tracks                 off"));
        assert!(off.contains("Advisories             off"));
        assert!(off.contains("Capital Corridor       kept"));

        // Defaults: every source points at the built-in browser worker.
        let worker = tracks(&[
            ("NJT_RAILDATA_USERNAME", "user"),
            ("NJT_RAILDATA_PASSWORD", "secret-pw"),
        ]);
        let notices = crate::config::AdvisoryConfig::from_map(|_| None).unwrap();
        let on = startup_banner(
            "http://x",
            bind,
            &access,
            Some(&notices),
            Some(&worker),
            true,
        );
        assert!(on.contains("Advisories             on (built-in browser worker)"));
        assert!(on.contains("Amtrak status        on (built-in browser worker)"));
        assert!(on.contains("NJ Transit RailData  on (8 stations)"));
        assert!(on.contains("Hartford Line board  on (NHV)"));
        assert!(on.contains("Capital Corridor       filtered out"));
        assert!(!on.contains("secret-pw"));
        assert!(!on.contains("127.0.0.1:8081"));

        let custom = tracks(&[(
            "AMTRAK_TRACKS_AMTRAK_URL",
            "http://track-fetcher:8081/tracks.json",
        )]);
        let custom = startup_banner("http://x", bind, &access, None, Some(&custom), false);
        assert!(custom.contains("Amtrak status        on (AMTRAK_TRACKS_AMTRAK_URL)"));
        assert!(!custom.contains("track-fetcher:8081"));

        let flagged_off = tracks(&[
            ("AMTRAK_TRACKS_AMTRAK", "off"),
            ("AMTRAK_TRACKS_HARTFORD", "off"),
        ]);
        let flagged_off =
            startup_banner("http://x", bind, &access, None, Some(&flagged_off), false);
        assert!(flagged_off.contains("Amtrak status        off: AMTRAK_TRACKS_AMTRAK=off"));
        assert!(flagged_off.contains("Hartford Line board  off: AMTRAK_TRACKS_HARTFORD=off"));
        assert!(flagged_off.contains("NJ Transit RailData  off: no NJT_RAILDATA credentials"));

        let emptied = tracks(&[
            ("AMTRAK_TRACKS_AMTRAK_URL", ""),
            ("AMTRAK_TRACKS_HARTFORD_URL", ""),
        ]);
        let emptied = startup_banner("http://x", bind, &access, None, Some(&emptied), false);
        assert!(emptied.contains("Amtrak status        off: AMTRAK_TRACKS_AMTRAK_URL is empty"));
        assert!(emptied.contains("Hartford Line board  off: AMTRAK_TRACKS_HARTFORD_URL is empty"));
    }
}
