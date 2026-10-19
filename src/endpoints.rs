//! Tells whoever started the service where its feed is.
//!
//! At startup the service prints its endpoints and which optional sources are on. When the first
//! generation is published it prints that generation's file URLs once, so a reader can fetch a
//! protobuf without composing the manifest lookup by hand. Only local endpoints and source names
//! are printed: no upstream URL, credential, or response text.

use crate::config::{AccessPolicy, TrackConfig};
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
pub fn startup_banner(
    base: &str,
    bind: SocketAddr,
    access: &AccessPolicy,
    advisories: bool,
    tracks: Option<&TrackConfig>,
) -> String {
    let mut lines = vec![
        format!("amtrak-gtfs-rt listening on {bind}"),
        format!("  Feed set (start here)  {base}/v1/feed-set.json"),
        format!(
            "  Generation files       {base}/v1/generations/{{id}}/\
             {{static.zip,trip-updates.pb,vehicle-positions.pb,alerts.pb}}"
        ),
        format!("  Readiness              {base}/readyz"),
        format!("  Liveness               {base}/livez"),
        format!("  Access                 {}", access.describe()),
        format!(
            "  Advisories             {}",
            if advisories { "on" } else { "off" }
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

fn track_sources(tracks: &TrackConfig) -> Vec<String> {
    let amtrak = if tracks.amtrak_url.is_some() {
        "on (track-fetcher sidecar)"
    } else {
        "off: AMTRAK_TRACKS_AMTRAK_URL is empty"
    };
    let raildata = match (
        tracks.njt_stations.is_empty(),
        tracks.raildata_credentials().is_some(),
    ) {
        (true, _) => "off: no stations".to_string(),
        (false, false) => "off: no NJT_RAILDATA credentials".to_string(),
        (false, true) => format!("on ({} stations)", tracks.njt_stations.len()),
    };
    let hartford = if tracks.hartford_url.is_some() {
        format!("on ({})", tracks.hartford_stop_id)
    } else {
        "off".to_string()
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
        let off = startup_banner("http://127.0.0.1:8080", bind, &access, false, None);
        assert!(off.contains("http://127.0.0.1:8080/v1/feed-set.json"));
        assert!(off.contains(
            "http://127.0.0.1:8080/v1/generations/{id}/\
             {static.zip,trip-updates.pb,vehicle-positions.pb,alerts.pb}"
        ));
        assert!(off.contains("http://127.0.0.1:8080/readyz"));
        assert!(off.contains("loopback clients only"));
        assert!(off.contains("Tracks                 off"));

        let wired = tracks(&[
            ("AMTRAK_TRACKS", "on"),
            (
                "AMTRAK_TRACKS_AMTRAK_URL",
                "http://track-fetcher:8081/tracks.json",
            ),
            ("NJT_RAILDATA_USERNAME", "user"),
            ("NJT_RAILDATA_PASSWORD", "secret-pw"),
        ]);
        let on = startup_banner("http://x", bind, &access, true, Some(&wired));
        assert!(on.contains("Amtrak status        on (track-fetcher sidecar)"));
        assert!(on.contains("NJ Transit RailData  on (8 stations)"));
        assert!(on.contains("Hartford Line board  on (NHV)"));
        assert!(!on.contains("secret-pw"));
        assert!(!on.contains("track-fetcher:8081"));

        let bare = tracks(&[("AMTRAK_TRACKS", "on")]);
        let bare = startup_banner("http://x", bind, &access, true, Some(&bare));
        assert!(bare.contains("Amtrak status        off: AMTRAK_TRACKS_AMTRAK_URL is empty"));
        assert!(bare.contains("NJ Transit RailData  off: no NJT_RAILDATA credentials"));
    }
}
