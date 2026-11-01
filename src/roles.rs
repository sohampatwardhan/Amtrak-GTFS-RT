//! VehiclePositions, TripUpdates, and Alerts process entry points.
//!
//! All three link this crate. Only VehiclePositions polls `getTrainsData`.
//! TripUpdates and Alerts decorate that snapshot. The public feed set changes
//! only after both decorations are on disk.

use crate::cohort::{self, FleetMeta};
use crate::config::Config;
use crate::endpoints::{base_url, startup_banner};
use crate::orchestrator::{
    CandidateValidator, GenerationBuilder, GenerationCommitter, SelectedBatch,
    StoreGenerationCommitter, ValidatedGeneration,
};
use crate::schedule::TrackScheduler;
use crate::serve::{self, AppState, SystemClock, TripInterest};
use crate::sources::advisories::fetch_advisory_alerts;
use crate::sources::tracks::apply_track_assignments;
use crate::sources::tracks::refresher::run_board_refresher;
use crate::sources::tracks::AssignmentStore;
use crate::sources::RtBatch;
use crate::static_gtfs::recover_static;
use crate::writer::{GenerationStore, PublishError, PublishedGeneration};
use crate::{initial_snapshots, supervise};
use async_trait::async_trait;
use gtfs_realtime::FeedMessage;
use prost::Message;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static FLEET_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Polls the fleet, writes the snapshot, and serves the public HTTP API.
///
/// # Errors
///
/// Returns the same startup errors as the combined process: configuration,
/// a static snapshot that cannot be loaded, or a bind failure.
///
/// # Panics
///
/// Does not panic. A poisoned lock is recovered by the store.
pub async fn run_vehicle_positions() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = Config::from_env()?.validate()?;
    let store =
        GenerationStore::open_with_retention(&config.output_dir, config.generation_retention)
            .await?;
    let validator: Arc<dyn crate::static_gtfs::StaticStandardsValidator> =
        Arc::new(crate::static_gtfs::CachedStaticValidator::new(
            crate::static_gtfs::MobilityDataStaticValidator::new(config.gtfs_validator_jar.clone()),
            config.output_dir.join("validator-cache"),
            config.validator_cache,
        ));
    let track_config = crate::config::TrackConfig::from_env()?;
    let observed = config
        .output_dir
        .join("tracks")
        .join("observed-platforms.txt");
    let platforms = track_config.enabled.then(|| {
        crate::static_augment::SharedPlatforms::load(track_config.platforms.clone(), observed)
    });
    let bootstrap_tables = platforms
        .as_ref()
        .map(crate::static_augment::SharedPlatforms::candidates)
        .unwrap_or_default();
    let snapshots = initial_snapshots(
        &store,
        &config.static_url,
        validator.as_ref(),
        &bootstrap_tables,
    )
    .await?;
    if let Some(platforms) = &platforms {
        let active = snapshots.active().await;
        platforms.set_known_stations(active.parsed.stops.keys().map(String::as_str));
    }
    let source: Box<dyn crate::sources::RtSource> = if config.filter_capital_corridor {
        Box::new(crate::CapitalCorridorFiltered(
            crate::sources::amtrak::AmtrakSource::new(),
        ))
    } else {
        Box::new(crate::sources::amtrak::AmtrakSource::new())
    };
    let sources = Arc::new(vec![source]);
    let fleet = Arc::new(FleetCommitter {
        output_dir: config.output_dir.clone(),
    });
    let scheduler = Arc::new(Mutex::new(TrackScheduler::new(
        std::env::var("AMTRAK_TRACKS_BASE_INTERVAL_SECS")
            .ok()
            .and_then(|value| value.parse().ok())
            .map(Duration::from_secs)
            .unwrap_or(crate::schedule::DEFAULT_BASE_INTERVAL),
        std::env::var("AMTRAK_TRACKS_DEBOUNCE_SECS")
            .ok()
            .and_then(|value| value.parse().ok())
            .map(Duration::from_secs)
            .unwrap_or(crate::schedule::DEFAULT_DEBOUNCE),
        std::env::var("AMTRAK_TRACKS_MIN_GAP_SECS")
            .ok()
            .and_then(|value| value.parse().ok())
            .map(Duration::from_secs)
            .unwrap_or(crate::schedule::DEFAULT_MIN_GAP),
        std::env::var("AMTRAK_TRACKS_MAX_EARLY")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(crate::schedule::DEFAULT_MAX_PER_FLUSH),
    )));
    let hint_scheduler = scheduler.clone();
    let hint_dir = config.output_dir.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let (trains, stations) = {
                let Ok(mut guard) = hint_scheduler.lock() else {
                    continue;
                };
                let now = std::time::Instant::now();
                (guard.take_early(now), guard.take_stations(now))
            };
            if trains.is_empty() && stations.is_empty() {
                continue;
            }
            let path = hint_dir.join("tracks").join("early-refresh.json");
            let body = serde_json::json!({"trains": trains, "stations": stations}).to_string();
            let _ = std::fs::create_dir_all(hint_dir.join("tracks"));
            let _ = std::fs::write(path, body);
        }
    });
    let enqueue = scheduler.clone();
    let app = serve::router(
        AppState::new(
            store.clone(),
            config.access_policy.clone(),
            config.freshness_limit,
            Arc::new(SystemClock),
        )
        .with_trip_interest(Arc::new(move |interest: TripInterest| {
            let Ok(mut guard) = enqueue.lock() else {
                return;
            };
            let now = std::time::Instant::now();
            guard.enqueue_trains(&interest.trains, now);
            for station in &interest.stations {
                guard.enqueue_station(station, now);
            }
        })),
    );
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    let base = base_url(config.bind_addr, config.public_url.as_deref());
    println!(
        "{}",
        startup_banner(&base, config.bind_addr, &config.access_policy, false, None,)
    );
    let assembler_store = store.clone();
    let assembler_dir = config.output_dir.clone();
    tokio::spawn(async move {
        assemble_forever(assembler_store, assembler_dir).await;
    });
    let poller_snapshots = snapshots.clone();
    let refresh_snapshots = snapshots;
    let poll_interval = config.poll_interval;
    let static_url = config.static_url.clone();
    let static_refresh_interval = config.static_refresh_interval;
    supervise(
        async move {
            crate::orchestrator::run_poller(sources, poller_snapshots, fleet, poll_interval).await;
            Ok(())
        },
        async move {
            crate::static_gtfs::run_snapshot_refresh(
                refresh_snapshots,
                static_url,
                static_refresh_interval,
                validator,
                platforms,
            )
            .await;
            Ok(())
        },
        async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .map_err(|_| crate::ServiceError::failed(crate::ServiceActivity::Http))
        },
    )
    .await?;
    Ok(())
}

/// Stamps tracks onto each fleet snapshot and writes the trip-update contribution.
///
/// # Errors
///
/// Returns a configuration error. Board and browser failures are logged and the
/// loop continues.
pub async fn run_trip_updates() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::try_init().ok();
    embed_browser("8081");
    let config = Config::from_env()?.validate()?;
    let track_config = crate::config::TrackConfig::from_env()?;
    let observed = config
        .output_dir
        .join("tracks")
        .join("observed-platforms.txt");
    let platforms =
        crate::static_augment::SharedPlatforms::load(track_config.platforms.clone(), observed);
    let assignments = Arc::new(AssignmentStore::new());
    if track_config.enabled {
        tokio::spawn(run_board_refresher(
            assignments.clone(),
            track_config.clone(),
            config.output_dir.join("tracks").join("raildata-token.json"),
            platforms.clone(),
        ));
    }
    let mut seen = String::new();
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let Some(id) = cohort::current_fleet_id(&config.output_dir) else {
            continue;
        };
        if id == seen {
            continue;
        }
        if stamp_trip_updates(
            &config.output_dir,
            &id,
            &assignments,
            &platforms,
            &track_config,
        )
        .is_ok()
        {
            seen = id;
        }
    }
}

/// Merges notices onto each fleet snapshot and writes the alert contribution.
///
/// # Errors
///
/// Returns a configuration error. A notice failure still writes the base alerts.
pub async fn run_alerts() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::try_init().ok();
    embed_browser("8080");
    let config = Config::from_env()?.validate()?;
    let advisory = crate::config::AdvisoryConfig::from_env()?;
    let mut seen = String::new();
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let Some(id) = cohort::current_fleet_id(&config.output_dir) else {
            continue;
        };
        if id == seen {
            continue;
        }
        if write_alerts(&config.output_dir, &id, &advisory)
            .await
            .is_ok()
        {
            seen = id;
        }
    }
}

struct FleetCommitter {
    output_dir: PathBuf,
}

#[async_trait]
impl GenerationCommitter for FleetCommitter {
    async fn publish(
        &self,
        candidate: ValidatedGeneration,
    ) -> Result<Arc<PublishedGeneration>, PublishError> {
        let id = format!(
            "{}-{}",
            candidate.generated_at_unix,
            FLEET_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let meta = FleetMeta {
            id: id.clone(),
            generated_at_unix: candidate.generated_at_unix,
            static_version: candidate.static_snapshot.version.clone(),
        };
        cohort::write_fleet(
            &self.output_dir,
            &meta,
            &candidate.trip_updates,
            &candidate.vehicle_positions,
            &candidate.alerts,
            &candidate.static_snapshot.zip,
        )
        .map_err(|error| PublishError(error.to_string()))?;
        Ok(Arc::new(PublishedGeneration {
            id: crate::orchestrator::GenerationId(id),
            manifest: candidate.manifest(
                crate::orchestrator::GenerationId("0-0".into()),
                crate::orchestrator::ArtifactUrls::default(),
            ),
            static_zip: candidate.static_snapshot.zip.clone(),
            trip_updates: candidate.trip_updates.clone(),
            vehicle_positions: candidate.vehicle_positions.clone(),
            alerts: candidate.alerts.clone(),
        }))
    }
}

async fn assemble_forever(store: GenerationStore, output_dir: PathBuf) {
    let committer = StoreGenerationCommitter::new(output_dir.clone(), store);
    let mut published = String::new();
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let Some(id) = cohort::current_fleet_id(&output_dir) else {
            continue;
        };
        if id == published || !cohort::contributions_ready(&output_dir, &id) {
            continue;
        }
        match assemble_one(&output_dir, &id).await {
            Ok(candidate) => match committer.publish(candidate).await {
                Ok(_) => published = id,
                Err(error) => tracing::warn!(%error, "cohort publish failed"),
            },
            Err(error) => tracing::warn!(%error, fleet_id = %id, "cohort is not publishable yet"),
        }
    }
}

async fn assemble_one(
    output_dir: &std::path::Path,
    id: &str,
) -> Result<ValidatedGeneration, String> {
    let meta = cohort::read_meta(output_dir, id)?;
    let zip = std::fs::read(cohort::fleet_file(output_dir, id, "static.zip"))
        .map_err(|error| error.to_string())?;
    let snapshot = recover_static(Arc::<[u8]>::from(zip), meta.static_version.clone())
        .map_err(|error| error.to_string())?
        .active()
        .await;
    let vehicles = decode_feed(&cohort::fleet_file(output_dir, id, "vehicle-positions.pb"))?;
    let stamped = decode_feed(&cohort::contribution_file(
        output_dir,
        id,
        "trip-updates.pb",
    ));
    let trips = match stamped {
        Ok(message) => message,
        Err(_) => decode_feed(&cohort::fleet_file(output_dir, id, "trip-updates.pb"))?,
    };
    let merged = decode_feed(&cohort::contribution_file(output_dir, id, "alerts.pb"));
    let alerts = match merged {
        Ok(message) => message,
        Err(_) => decode_feed(&cohort::fleet_file(output_dir, id, "alerts.pb"))?,
    };
    let generated_at = UNIX_EPOCH + Duration::from_secs(meta.generated_at_unix);
    let candidate = GenerationBuilder::build(
        snapshot,
        SelectedBatch {
            source_name: "amtrak",
            batch: RtBatch {
                trip_updates: trips,
                vehicle_positions: vehicles,
                alerts,
            },
        },
        generated_at,
    )
    .map_err(|error| error.to_string())?;
    CandidateValidator::validate(candidate).map_err(|error| error.to_string())
}

fn stamp_trip_updates(
    output_dir: &std::path::Path,
    id: &str,
    assignments: &AssignmentStore,
    platforms: &crate::static_augment::SharedPlatforms,
    tracks: &crate::config::TrackConfig,
) -> Result<(), String> {
    let path = cohort::fleet_file(output_dir, id, "trip-updates.pb");
    let mut message = decode_feed(&path)?;
    let zip = std::fs::read(cohort::fleet_file(output_dir, id, "static.zip"))
        .map_err(|error| error.to_string())?;
    let gtfs =
        gtfs_structures::Gtfs::from_reader(Cursor::new(zip)).map_err(|error| error.to_string())?;
    if tracks.enabled {
        let board = assignments.fresh_board(SystemTime::now(), tracks.max_age);
        let mut batch = RtBatch {
            trip_updates: message,
            vehicle_positions: FeedMessage::default(),
            alerts: FeedMessage::default(),
        };
        let unlisted = Mutex::new(std::collections::HashSet::new());
        apply_track_assignments(
            &mut batch,
            &gtfs,
            &board,
            &platforms.snapshot(),
            SystemTime::now(),
            &unlisted,
        );
        message = batch.trip_updates;
    }
    cohort::write_contribution(output_dir, id, "trip-updates.pb", &message.encode_to_vec())
        .map_err(|error| error.to_string())
}

async fn write_alerts(
    output_dir: &std::path::Path,
    id: &str,
    advisory: &crate::config::AdvisoryConfig,
) -> Result<(), String> {
    let mut message = decode_feed(&cohort::fleet_file(output_dir, id, "alerts.pb"))?;
    if advisory.enabled {
        let zip = std::fs::read(cohort::fleet_file(output_dir, id, "static.zip"))
            .map_err(|error| error.to_string())?;
        let gtfs = gtfs_structures::Gtfs::from_reader(Cursor::new(zip))
            .map_err(|error| error.to_string())?;
        let mut extra = fetch_advisory_alerts(&reqwest::Client::new(), &gtfs, &advisory.url).await;
        message.entity.append(&mut extra);
    }
    cohort::write_contribution(output_dir, id, "alerts.pb", &message.encode_to_vec())
        .map_err(|error| error.to_string())
}

fn decode_feed(path: &std::path::Path) -> Result<FeedMessage, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    FeedMessage::decode(bytes.as_slice()).map_err(|error| error.to_string())
}

fn embed_browser(port: &str) {
    if !std::path::Path::new("/app/fetcher").is_dir() {
        return;
    }
    let mut command = std::process::Command::new("python");
    command
        .args(["-m", "fetcher"])
        .current_dir("/app")
        .env("LISTEN_PORT", port);
    if let Err(error) = command.spawn() {
        tracing::warn!(%error, "browser worker did not start");
    }
}
