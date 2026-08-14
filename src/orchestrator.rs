use crate::sources::{RtBatch, RtSource};
use crate::static_gtfs::StaticSnapshotState;
use crate::writer::{GenerationPublisher, GenerationStore, PublishError, PublishedGeneration};
use async_trait::async_trait;
use gtfs_realtime::{feed_header, FeedEntity, FeedHeader, FeedMessage};
use gtfs_structures::{Gtfs, RouteType};
use prost::Message;
use std::collections::HashSet;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// One accepted static schedule and the exact ZIP bytes that produced it.
pub struct StaticSnapshot {
    pub version: String,
    pub parsed: Arc<Gtfs>,
    pub zip: Arc<[u8]>,
}

/// A non-empty realtime batch paired with the source that supplied it.
#[derive(Clone, Debug)]
pub struct SelectedBatch {
    pub source_name: &'static str,
    pub batch: RtBatch,
}

/// Failure to obtain a non-empty batch from the configured source chain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshError(pub String);

impl std::fmt::Display for RefreshError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for RefreshError {}

/// Allowlisted processing stage recorded for one refresh completion event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshStage {
    Source,
    EmptyCandidate,
    Build,
    Validate,
    Publish,
    Commit,
}

impl RefreshStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::EmptyCandidate => "empty_candidate",
            Self::Build => "build",
            Self::Validate => "validate",
            Self::Publish => "publish",
            Self::Commit => "commit",
        }
    }
}

/// Credential-free summary emitted exactly once for each realtime refresh.
///
/// The closed field set deliberately excludes error strings, URLs, headers,
/// configuration values, and source payloads so operational logs cannot copy
/// credentials or access secrets by accident.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshTelemetry {
    pub outcome: &'static str,
    pub stage: RefreshStage,
    pub source: Option<&'static str>,
    pub generation_id: Option<GenerationId>,
    pub static_version: String,
    pub duration_ms: u64,
    pub entity_counts: EntityCounts,
}

/// Immutable lookup key assigned when a validated candidate is published.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct GenerationId(pub String);

impl FromStr for GenerationId {
    type Err = ();

    /// Parses the closed `<unix-nanoseconds>-<process-counter>` grammar.
    ///
    /// Both components must be non-empty ASCII decimal integers that fit the
    /// publisher's numeric domains. Restricting the grammar here ensures HTTP
    /// identifiers remain lookup keys rather than filesystem input.
    ///
    /// # Errors
    ///
    /// Returns `()` when the value has the wrong delimiter/count, contains
    /// non-ASCII digits, has an empty component, or exceeds numeric bounds.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (timestamp, counter) = value.split_once('-').ok_or(())?;
        if timestamp.is_empty()
            || counter.is_empty()
            || timestamp.bytes().any(|byte| !byte.is_ascii_digit())
            || counter.bytes().any(|byte| !byte.is_ascii_digit())
            || timestamp.parse::<u128>().is_err()
            || counter.parse::<u64>().is_err()
        {
            return Err(());
        }
        Ok(Self(value.to_owned()))
    }
}

/// Per-product entity counts included in the feed-set manifest.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EntityCounts {
    pub trip_updates: usize,
    pub vehicle_positions: usize,
    pub alerts: usize,
}

/// Generation-pinned artifact URLs exposed to controlled consumers.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArtifactUrls {
    pub static_zip: String,
    pub trip_updates: String,
    pub vehicle_positions: String,
    pub alerts: String,
}

/// Discovery metadata for one coherent static and realtime generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeedSetManifest {
    pub generation_id: GenerationId,
    pub generated_at_unix: u64,
    pub static_version: String,
    pub source: String,
    pub entity_counts: EntityCounts,
    pub urls: ArtifactUrls,
}

/// A normalized, static-bound generation before wire validation.
pub struct CandidateGeneration {
    pub static_snapshot: Arc<StaticSnapshot>,
    pub generated_at_unix: u64,
    pub source_name: &'static str,
    pub trip_updates: FeedMessage,
    pub vehicle_positions: FeedMessage,
    pub alerts: FeedMessage,
}

/// A candidate whose three products independently decode and satisfy invariants.
pub struct ValidatedGeneration {
    pub static_snapshot: Arc<StaticSnapshot>,
    pub generated_at_unix: u64,
    pub source_name: &'static str,
    pub trip_updates: Arc<[u8]>,
    pub vehicle_positions: Arc<[u8]>,
    pub alerts: Arc<[u8]>,
    pub entity_counts: EntityCounts,
}

impl ValidatedGeneration {
    /// Creates discovery metadata from the exact validated generation fields.
    ///
    /// Keeping this construction on the validated value prevents a publisher
    /// from accidentally mixing counts, timestamps, or static versions from a
    /// different refresh.
    pub fn manifest(&self, generation_id: GenerationId, urls: ArtifactUrls) -> FeedSetManifest {
        FeedSetManifest {
            generation_id,
            generated_at_unix: self.generated_at_unix,
            static_version: self.static_snapshot.version.clone(),
            source: self.source_name.to_string(),
            entity_counts: self.entity_counts,
            urls,
        }
    }
}

/// Candidate construction failure that leaves the prior generation unchanged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildError(pub String);

impl std::fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for BuildError {}

/// Wire or semantic validation failure for a complete candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationError(pub String);

impl std::fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ValidationError {}

/// Tries sources in order and returns the first successful non-empty batch.
///
/// # Errors
///
/// Returns [`RefreshError`] after every source fails or returns no entities.
pub async fn select_batch(
    sources: &[Box<dyn RtSource>],
    gtfs: &Gtfs,
) -> Result<SelectedBatch, RefreshError> {
    for source in sources {
        match source.fetch(gtfs).await {
            Ok(batch) if !batch.is_empty() => {
                return Ok(SelectedBatch {
                    source_name: source.name(),
                    batch,
                });
            }
            Ok(_) | Err(_) => {}
        }
    }
    Err(RefreshError(
        "no configured source returned a non-empty batch".into(),
    ))
}

/// Builds normalized FULL_DATASET products against one static snapshot.
pub struct GenerationBuilder;

impl GenerationBuilder {
    /// Partitions payloads, filters unresolved references, and stamps one header.
    ///
    /// `is_deleted` is removed because deletion semantics are invalid for the
    /// selected `FULL_DATASET` incrementality. Unmatched references are omitted
    /// so consumers are never told that a nonexistent scheduled trip is live.
    ///
    /// # Errors
    ///
    /// Returns [`BuildError`] for an invalid generation time or empty static
    /// version. Individual malformed source entities are safely omitted.
    pub fn build(
        static_snapshot: Arc<StaticSnapshot>,
        selected: SelectedBatch,
        generated_at: SystemTime,
    ) -> Result<CandidateGeneration, BuildError> {
        if static_snapshot.version.trim().is_empty() {
            return Err(BuildError("static version is empty".into()));
        }
        let generated_at_unix = generated_at
            .duration_since(UNIX_EPOCH)
            .map_err(|_| BuildError("generation time precedes Unix epoch".into()))?
            .as_secs();
        let header = normalized_header(generated_at_unix, &static_snapshot.version);
        let mut trip_updates = FeedMessage {
            header: header.clone(),
            entity: Vec::new(),
        };
        let mut vehicle_positions = FeedMessage {
            header: header.clone(),
            entity: Vec::new(),
        };
        let mut alerts = FeedMessage {
            header,
            entity: Vec::new(),
        };
        // Some upstream adapters repeat the same unified entity in more than
        // one nominal product message. Partition each logical entity once per
        // output while still allowing a later valid copy after an invalid one.
        let mut trip_ids = HashSet::new();
        let mut vehicle_ids = HashSet::new();
        let mut alert_ids = HashSet::new();

        for entity in selected
            .batch
            .trip_updates
            .entity
            .into_iter()
            .chain(selected.batch.vehicle_positions.entity)
            .chain(selected.batch.alerts.entity)
        {
            if let Some(mut update) = entity.trip_update.clone() {
                if valid_trip_descriptor(&update.trip, &static_snapshot.parsed) {
                    update.stop_time_update.retain(|stop| {
                        valid_stop_time_update(stop, &update.trip, &static_snapshot.parsed)
                    });
                    if (!update.stop_time_update.is_empty() || trip_update_allows_empty(&update))
                        && trip_ids.insert(entity.id.clone())
                    {
                        trip_updates.entity.push(FeedEntity {
                            id: entity.id.clone(),
                            is_deleted: None,
                            trip_update: Some(update),
                            ..Default::default()
                        });
                    }
                }
            }
            if let Some(vehicle) = entity.vehicle.clone() {
                let trip_valid = vehicle
                    .trip
                    .as_ref()
                    .is_some_and(|trip| valid_trip_descriptor(trip, &static_snapshot.parsed));
                let stop_valid = vehicle.stop_id.as_ref().is_none_or(|id| {
                    vehicle
                        .trip
                        .as_ref()
                        .is_some_and(|trip| valid_stop_for_trip(id, trip, &static_snapshot.parsed))
                });
                let position_valid = vehicle.position.as_ref().is_some_and(|position| {
                    position.latitude.is_finite()
                        && position.longitude.is_finite()
                        && (-90.0..=90.0).contains(&position.latitude)
                        && (-180.0..=180.0).contains(&position.longitude)
                });
                if trip_valid
                    && stop_valid
                    && position_valid
                    && vehicle_ids.insert(entity.id.clone())
                {
                    vehicle_positions.entity.push(FeedEntity {
                        id: entity.id.clone(),
                        is_deleted: None,
                        vehicle: Some(vehicle),
                        ..Default::default()
                    });
                }
            }
            if let Some(mut alert) = entity.alert {
                alert
                    .informed_entity
                    .retain(|selector| valid_alert_selector(selector, &static_snapshot.parsed));
                if !alert.informed_entity.is_empty()
                    && alert_has_text(&alert)
                    && alert_ids.insert(entity.id.clone())
                {
                    alerts.entity.push(FeedEntity {
                        id: entity.id,
                        is_deleted: None,
                        alert: Some(alert),
                        ..Default::default()
                    });
                }
            }
        }

        Ok(CandidateGeneration {
            static_snapshot,
            generated_at_unix,
            source_name: selected.source_name,
            trip_updates,
            vehicle_positions,
            alerts,
        })
    }
}

/// Validates the exact protobuf bytes that publication will persist.
pub struct CandidateValidator;

impl CandidateValidator {
    /// Encodes and independently decodes each product before accepting it.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError`] for duplicate IDs, mixed payloads, header
    /// divergence, or any reference/geometry/text invariant missed by building.
    pub fn validate(
        candidate: CandidateGeneration,
    ) -> Result<ValidatedGeneration, ValidationError> {
        let expected_header = normalized_header(
            candidate.generated_at_unix,
            &candidate.static_snapshot.version,
        );
        let trip_updates = validate_product(
            candidate.trip_updates,
            Product::TripUpdate,
            &expected_header,
            &candidate.static_snapshot.parsed,
        )?;
        let vehicle_positions = validate_product(
            candidate.vehicle_positions,
            Product::Vehicle,
            &expected_header,
            &candidate.static_snapshot.parsed,
        )?;
        let alerts = validate_product(
            candidate.alerts,
            Product::Alert,
            &expected_header,
            &candidate.static_snapshot.parsed,
        )?;
        let entity_counts = EntityCounts {
            trip_updates: trip_updates.1,
            vehicle_positions: vehicle_positions.1,
            alerts: alerts.1,
        };
        Ok(ValidatedGeneration {
            static_snapshot: candidate.static_snapshot,
            generated_at_unix: candidate.generated_at_unix,
            source_name: candidate.source_name,
            trip_updates: trip_updates.0,
            vehicle_positions: vehicle_positions.0,
            alerts: alerts.0,
            entity_counts,
        })
    }
}

trait GenerationProcessor: Send + Sync {
    fn build_and_validate(
        &self,
        snapshot: Arc<StaticSnapshot>,
        selected: SelectedBatch,
        generated_at: SystemTime,
    ) -> Result<ValidatedGeneration, RefreshStage>;
}

struct DefaultGenerationProcessor;

impl GenerationProcessor for DefaultGenerationProcessor {
    fn build_and_validate(
        &self,
        snapshot: Arc<StaticSnapshot>,
        selected: SelectedBatch,
        generated_at: SystemTime,
    ) -> Result<ValidatedGeneration, RefreshStage> {
        let candidate = GenerationBuilder::build(snapshot, selected, generated_at)
            .map_err(|_| RefreshStage::Build)?;
        CandidateValidator::validate(candidate).map_err(|_| RefreshStage::Validate)
    }
}

/// Publication boundary used by the recoverable poller.
///
/// Implementations must not report success until the generation is durable and
/// visible through their store. This small boundary makes publication failure
/// independently injectable without weakening the real publisher contract.
#[async_trait]
pub trait GenerationCommitter: Send + Sync {
    /// Durably publishes one already validated candidate.
    ///
    /// # Errors
    ///
    /// Returns [`PublishError`] without changing the visible generation when
    /// any persistence or commit boundary fails.
    async fn publish(
        &self,
        candidate: ValidatedGeneration,
    ) -> Result<Arc<PublishedGeneration>, PublishError>;
}

/// Connects the generation refresh pipeline to one opened durable store.
pub struct StoreGenerationCommitter {
    output_dir: PathBuf,
    store: GenerationStore,
}

impl StoreGenerationCommitter {
    /// Binds publication to the exact directory used to open `store`.
    ///
    /// # Errors
    ///
    /// Publication later returns [`PublishError`] if the path does not resolve
    /// to that store or any durability step fails.
    pub fn new(output_dir: PathBuf, store: GenerationStore) -> Self {
        Self { output_dir, store }
    }
}

#[async_trait]
impl GenerationCommitter for StoreGenerationCommitter {
    async fn publish(
        &self,
        candidate: ValidatedGeneration,
    ) -> Result<Arc<PublishedGeneration>, PublishError> {
        GenerationPublisher::publish(&self.output_dir, &self.store, candidate).await
    }
}

/// Runs one recoverable realtime refresh against the pending static snapshot
/// when present, otherwise against the active snapshot.
///
/// Static state is promoted only after the complete generation has durably
/// published. Every failure returns an allowlisted telemetry record and leaves
/// both the generation store and pending-static state available for a retry.
pub async fn refresh_generation_once(
    sources: &[Box<dyn RtSource>],
    snapshots: &StaticSnapshotState,
    committer: &dyn GenerationCommitter,
    generated_at: SystemTime,
) -> RefreshTelemetry {
    refresh_generation_once_with(
        sources,
        snapshots,
        committer,
        &DefaultGenerationProcessor,
        generated_at,
    )
    .await
}

async fn refresh_generation_once_with(
    sources: &[Box<dyn RtSource>],
    snapshots: &StaticSnapshotState,
    committer: &dyn GenerationCommitter,
    processor: &dyn GenerationProcessor,
    generated_at: SystemTime,
) -> RefreshTelemetry {
    let started = Instant::now();
    let (snapshot, was_pending) = snapshots.candidate().await;
    let selected = match select_batch(sources, &snapshot.parsed).await {
        Ok(selected) => selected,
        Err(_) => {
            return finish_refresh(
                "failure",
                RefreshStage::Source,
                None,
                None,
                &snapshot.version,
                EntityCounts::default(),
                started,
            );
        }
    };
    let source = selected.source_name;
    let validated = match processor.build_and_validate(snapshot.clone(), selected, generated_at) {
        Ok(validated) => validated,
        Err(stage) => {
            return finish_refresh(
                "failure",
                stage,
                Some(source),
                None,
                &snapshot.version,
                EntityCounts::default(),
                started,
            );
        }
    };
    let counts = validated.entity_counts;
    if counts == EntityCounts::default() {
        return finish_refresh(
            "failure",
            RefreshStage::EmptyCandidate,
            Some(source),
            None,
            &snapshot.version,
            counts,
            started,
        );
    }
    let published = match committer.publish(validated).await {
        Ok(published) => published,
        Err(_) => {
            return finish_refresh(
                "failure",
                RefreshStage::Publish,
                Some(source),
                None,
                &snapshot.version,
                counts,
                started,
            );
        }
    };
    if was_pending {
        snapshots.promote_committed(&snapshot).await;
    }
    finish_refresh(
        "success",
        RefreshStage::Commit,
        Some(source),
        Some(published.id.clone()),
        &snapshot.version,
        counts,
        started,
    )
}

fn finish_refresh(
    outcome: &'static str,
    stage: RefreshStage,
    source: Option<&'static str>,
    generation_id: Option<GenerationId>,
    static_version: &str,
    entity_counts: EntityCounts,
    started: Instant,
) -> RefreshTelemetry {
    let telemetry = RefreshTelemetry {
        outcome,
        stage,
        source,
        generation_id,
        static_version: static_version.to_owned(),
        duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        entity_counts,
    };
    if outcome == "success" {
        tracing::info!(
            outcome = telemetry.outcome,
            stage = telemetry.stage.as_str(),
            source = telemetry.source.unwrap_or("none"),
            generation_id = telemetry
                .generation_id
                .as_ref()
                .map_or("none", |value| value.0.as_str()),
            static_version = telemetry.static_version,
            duration_ms = telemetry.duration_ms,
            trip_updates = telemetry.entity_counts.trip_updates,
            vehicle_positions = telemetry.entity_counts.vehicle_positions,
            alerts = telemetry.entity_counts.alerts,
            "realtime refresh"
        );
    } else {
        tracing::warn!(
            outcome = telemetry.outcome,
            stage = telemetry.stage.as_str(),
            source = telemetry.source.unwrap_or("none"),
            generation_id = "none",
            static_version = telemetry.static_version,
            duration_ms = telemetry.duration_ms,
            trip_updates = telemetry.entity_counts.trip_updates,
            vehicle_positions = telemetry.entity_counts.vehicle_positions,
            alerts = telemetry.entity_counts.alerts,
            "realtime refresh"
        );
    }
    telemetry
}

/// Polls indefinitely, preserving last-good state after every recoverable
/// source, conversion, validation, empty-candidate, or publication failure.
pub async fn run_poller(
    sources: Arc<Vec<Box<dyn RtSource>>>,
    snapshots: StaticSnapshotState,
    committer: Arc<dyn GenerationCommitter>,
    interval: Duration,
) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        ticker.tick().await;
        refresh_generation_once(
            sources.as_slice(),
            &snapshots,
            committer.as_ref(),
            SystemTime::now(),
        )
        .await;
    }
}

#[derive(Clone, Copy)]
enum Product {
    TripUpdate,
    Vehicle,
    Alert,
}

fn validate_product(
    message: FeedMessage,
    product: Product,
    expected_header: &FeedHeader,
    gtfs: &Gtfs,
) -> Result<(Arc<[u8]>, usize), ValidationError> {
    let bytes: Arc<[u8]> = message.encode_to_vec().into();
    let decoded = FeedMessage::decode(bytes.as_ref())
        .map_err(|error| ValidationError(format!("protobuf decode failed: {error}")))?;
    if &decoded.header != expected_header {
        return Err(ValidationError("feed header diverged".into()));
    }
    let mut ids = HashSet::new();
    for entity in &decoded.entity {
        if entity.id.is_empty() || !ids.insert(entity.id.as_str()) || entity.is_deleted.is_some() {
            return Err(ValidationError(
                "invalid or duplicate entity ID/deletion".into(),
            ));
        }
        let payload_count = usize::from(entity.trip_update.is_some())
            + usize::from(entity.vehicle.is_some())
            + usize::from(entity.alert.is_some());
        if payload_count != 1
            || entity.shape.is_some()
            || entity.stop.is_some()
            || entity.trip_modifications.is_some()
        {
            return Err(ValidationError(
                "entity does not have exactly one payload".into(),
            ));
        }
        match product {
            Product::TripUpdate => {
                let update = entity
                    .trip_update
                    .as_ref()
                    .ok_or_else(|| ValidationError("wrong trip-update payload".into()))?;
                if !valid_trip_descriptor(&update.trip, gtfs)
                    || (update.stop_time_update.is_empty() && !trip_update_allows_empty(update))
                    || update
                        .stop_time_update
                        .iter()
                        .any(|stop| !valid_stop_time_update(stop, &update.trip, gtfs))
                {
                    return Err(ValidationError("unresolved trip-update reference".into()));
                }
            }
            Product::Vehicle => {
                let vehicle = entity
                    .vehicle
                    .as_ref()
                    .ok_or_else(|| ValidationError("wrong vehicle payload".into()))?;
                let position = vehicle
                    .position
                    .as_ref()
                    .ok_or_else(|| ValidationError("vehicle has no position".into()))?;
                if !vehicle
                    .trip
                    .as_ref()
                    .is_some_and(|trip| valid_trip_descriptor(trip, gtfs))
                    || !position.latitude.is_finite()
                    || !position.longitude.is_finite()
                    || !(-90.0..=90.0).contains(&position.latitude)
                    || !(-180.0..=180.0).contains(&position.longitude)
                    || vehicle.stop_id.as_ref().is_some_and(|id| {
                        vehicle
                            .trip
                            .as_ref()
                            .is_none_or(|trip| !valid_stop_for_trip(id, trip, gtfs))
                    })
                {
                    return Err(ValidationError("invalid vehicle semantics".into()));
                }
            }
            Product::Alert => {
                let alert = entity
                    .alert
                    .as_ref()
                    .ok_or_else(|| ValidationError("wrong alert payload".into()))?;
                if alert.informed_entity.is_empty()
                    || !alert_has_text(alert)
                    || alert
                        .informed_entity
                        .iter()
                        .any(|selector| !valid_alert_selector(selector, gtfs))
                {
                    return Err(ValidationError("invalid alert semantics".into()));
                }
            }
        }
    }
    Ok((bytes, decoded.entity.len()))
}

fn normalized_header(timestamp: u64, version: &str) -> FeedHeader {
    FeedHeader {
        gtfs_realtime_version: "2.0".into(),
        incrementality: Some(feed_header::Incrementality::FullDataset as i32),
        timestamp: Some(timestamp),
        feed_version: Some(version.into()),
    }
}

fn valid_trip_descriptor(trip: &gtfs_realtime::TripDescriptor, gtfs: &Gtfs) -> bool {
    let Some(trip_id) = trip.trip_id.as_ref() else {
        return false;
    };
    let Some(static_trip) = gtfs.trips.get(trip_id) else {
        return false;
    };
    trip.route_id.as_ref().is_none_or(|route_id| {
        route_id == &static_trip.route_id && gtfs.routes.contains_key(route_id)
    })
}

fn valid_alert_selector(selector: &gtfs_realtime::EntitySelector, gtfs: &Gtfs) -> bool {
    let has_target = selector.route_id.is_some()
        || selector.stop_id.is_some()
        || selector.trip.is_some()
        || selector.agency_id.is_some()
        || selector.route_type.is_some();
    if !has_target {
        return false;
    }
    if selector
        .trip
        .as_ref()
        .is_some_and(|trip| !valid_trip_descriptor(trip, gtfs))
    {
        return false;
    }
    let combined_trip = selector
        .trip
        .as_ref()
        .and_then(|descriptor| descriptor.trip_id.as_ref())
        .and_then(|id| gtfs.trips.get(id));
    let agency_exists = selector.agency_id.as_ref().is_none_or(|id| {
        gtfs.agencies
            .iter()
            .any(|agency| agency.id.as_ref() == Some(id))
    });
    if !agency_exists
        || selector
            .route_type
            .is_some_and(|value| !valid_route_type(value))
    {
        return false;
    }
    let route_matches_agency = |route: &gtfs_structures::Route, agency_id: &str| {
        route.agency_id.as_ref().map_or_else(
            || gtfs.agencies.len() == 1 && gtfs.agencies[0].id.as_deref() == Some(agency_id),
            |route_agency| route_agency == agency_id,
        )
    };
    let matching_route_ids: HashSet<&str> = gtfs
        .routes
        .values()
        .filter(|route| {
            selector.route_id.as_ref().is_none_or(|id| route.id == *id)
                && combined_trip.is_none_or(|trip| route.id == trip.route_id)
                && selector
                    .route_type
                    .is_none_or(|value| canonical_route_type(route.route_type) == value)
                && selector
                    .agency_id
                    .as_deref()
                    .is_none_or(|id| route_matches_agency(route, id))
        })
        .map(|route| route.id.as_str())
        .collect();
    let needs_matching_route = selector.route_id.is_some()
        || combined_trip.is_some()
        || selector.route_type.is_some()
        || (selector.agency_id.is_some() && selector.stop_id.is_some());
    if needs_matching_route && matching_route_ids.is_empty() {
        return false;
    }
    selector.stop_id.as_ref().is_none_or(|stop_id| {
        gtfs.stops.contains_key(stop_id)
            && if let Some(trip) = combined_trip {
                trip.stop_times.iter().any(|time| time.stop.id == *stop_id)
            } else if needs_matching_route {
                gtfs.trips.values().any(|trip| {
                    matching_route_ids.contains(trip.route_id.as_str())
                        && trip.stop_times.iter().any(|time| time.stop.id == *stop_id)
                })
            } else {
                true
            }
    })
}

fn valid_stop_time_update(
    update: &gtfs_realtime::trip_update::StopTimeUpdate,
    descriptor: &gtfs_realtime::TripDescriptor,
    gtfs: &Gtfs,
) -> bool {
    let Some(trip_id) = descriptor.trip_id.as_ref() else {
        return false;
    };
    let Some(trip) = gtfs.trips.get(trip_id) else {
        return false;
    };
    let matching_sequence = update.stop_sequence.and_then(|sequence| {
        trip.stop_times
            .iter()
            .find(|time| time.stop_sequence == sequence)
    });
    let id_matches: Vec<_> = update
        .stop_id
        .as_ref()
        .map(|id| {
            trip.stop_times
                .iter()
                .filter(|time| time.stop.id == *id)
                .collect()
        })
        .unwrap_or_default();
    let reference_valid = match (update.stop_sequence, update.stop_id.as_ref()) {
        (None, None) => false,
        (Some(_), None) => matching_sequence.is_some(),
        (None, Some(_)) => id_matches.len() == 1,
        (Some(_), Some(id)) => matching_sequence.is_some_and(|time| time.stop.id == *id),
    };
    if !reference_valid {
        return false;
    }

    let assigned_stop = update
        .stop_time_properties
        .as_ref()
        .and_then(|properties| properties.assigned_stop_id.as_ref());
    let assignment_valid = assigned_stop.is_none_or(|assigned_id| {
        update.stop_sequence.is_some()
            && gtfs.stops.contains_key(assigned_id)
            && update
                .stop_id
                .as_ref()
                .is_none_or(|stop_id| stop_id == assigned_id)
    });
    if !assignment_valid {
        return false;
    }

    use gtfs_realtime::trip_update::stop_time_update::ScheduleRelationship;
    let relationship = update
        .schedule_relationship
        .unwrap_or(ScheduleRelationship::Scheduled as i32);
    let valid_event = |event: &gtfs_realtime::trip_update::StopTimeEvent| {
        event.delay.is_some() || event.time.is_some()
    };
    let events_valid = update.arrival.as_ref().is_none_or(&valid_event)
        && update.departure.as_ref().is_none_or(&valid_event);
    let events_present = update.arrival.is_some() || update.departure.is_some();
    let event_semantics = match ScheduleRelationship::try_from(relationship) {
        Ok(ScheduleRelationship::Scheduled | ScheduleRelationship::Unscheduled) => events_present,
        Ok(ScheduleRelationship::Skipped) => true,
        Ok(ScheduleRelationship::NoData) => !events_present,
        Err(_) => false,
    };
    let trip_relationship_consistent = relationship != ScheduleRelationship::Unscheduled as i32
        || descriptor.schedule_relationship
            == Some(gtfs_realtime::trip_descriptor::ScheduleRelationship::Unscheduled as i32);
    let assignment_semantics = assigned_stop
        .is_none_or(|_| events_present || relationship == ScheduleRelationship::NoData as i32);
    event_semantics && events_valid && trip_relationship_consistent && assignment_semantics
}

fn trip_update_allows_empty(update: &gtfs_realtime::TripUpdate) -> bool {
    use gtfs_realtime::trip_descriptor::ScheduleRelationship;
    matches!(
        update
            .trip
            .schedule_relationship
            .and_then(|value| ScheduleRelationship::try_from(value).ok()),
        Some(ScheduleRelationship::Canceled | ScheduleRelationship::Deleted)
    )
}

fn valid_route_type(value: i32) -> bool {
    matches!(value, 0..=7 | 11..=12)
}

fn canonical_route_type(route_type: RouteType) -> i32 {
    match route_type {
        RouteType::Tramway => 0,
        RouteType::Subway => 1,
        RouteType::Rail => 2,
        RouteType::Bus => 3,
        RouteType::Ferry => 4,
        RouteType::CableCar => 5,
        RouteType::Gondola => 6,
        RouteType::Funicular => 7,
        RouteType::Coach => 200,
        RouteType::Air => 1100,
        RouteType::Taxi => 1500,
        RouteType::Other(value) => i32::from(value),
    }
}

fn valid_stop_for_trip(
    stop_id: &str,
    descriptor: &gtfs_realtime::TripDescriptor,
    gtfs: &Gtfs,
) -> bool {
    descriptor
        .trip_id
        .as_ref()
        .and_then(|trip_id| gtfs.trips.get(trip_id))
        .is_some_and(|trip| trip.stop_times.iter().any(|time| time.stop.id == stop_id))
}

fn alert_has_text(alert: &gtfs_realtime::Alert) -> bool {
    [&alert.header_text, &alert.description_text]
        .into_iter()
        .flatten()
        .flat_map(|text| &text.translation)
        .any(|translation| !translation.text.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::mock::{batch_with, Behavior, MockSource};
    use gtfs_realtime::{
        translated_string, trip_update, Alert, EntitySelector, Position, TranslatedString,
        TripDescriptor, TripUpdate, VehiclePosition,
    };
    use gtfs_structures::{Agency, RawStopTime, Route, Stop, StopTime, Trip};

    fn sources(behaviors: Vec<(&'static str, Behavior)>) -> Vec<Box<dyn RtSource>> {
        behaviors
            .into_iter()
            .map(|(name, behavior)| Box::new(MockSource { name, behavior }) as Box<dyn RtSource>)
            .collect()
    }

    fn static_snapshot() -> Arc<StaticSnapshot> {
        let mut gtfs = Gtfs::default();
        gtfs.routes.insert(
            "route".into(),
            Route {
                id: "route".into(),
                agency_id: Some("agency-a".into()),
                ..Default::default()
            },
        );
        gtfs.agencies.push(Agency {
            id: Some("agency-a".into()),
            ..Default::default()
        });
        gtfs.agencies.push(Agency {
            id: Some("agency-b".into()),
            ..Default::default()
        });
        let stop = Arc::new(Stop {
            id: "stop".into(),
            ..Default::default()
        });
        gtfs.stops.insert("stop".into(), stop.clone());
        gtfs.stops.insert(
            "platform".into(),
            Arc::new(Stop {
                id: "platform".into(),
                ..Default::default()
            }),
        );
        let raw_stop_time = RawStopTime {
            stop_sequence: 1,
            ..Default::default()
        };
        gtfs.trips.insert(
            "trip".into(),
            Trip {
                id: "trip".into(),
                route_id: "route".into(),
                stop_times: vec![StopTime::from(raw_stop_time, stop)],
                ..Default::default()
            },
        );
        Arc::new(StaticSnapshot {
            version: "STATIC-V1".into(),
            parsed: Arc::new(gtfs),
            zip: Arc::from(&b"zip"[..]),
        })
    }

    fn descriptor() -> TripDescriptor {
        TripDescriptor {
            trip_id: Some("trip".into()),
            route_id: Some("route".into()),
            ..Default::default()
        }
    }

    fn mixed_batch() -> RtBatch {
        let alert = Alert {
            informed_entity: vec![EntitySelector {
                route_id: Some("route".into()),
                ..Default::default()
            }],
            header_text: Some(TranslatedString {
                translation: vec![translated_string::Translation {
                    text: "Service notice".into(),
                    language: Some("en".into()),
                }],
            }),
            ..Default::default()
        };
        let mixed = FeedEntity {
            id: "mixed".into(),
            is_deleted: Some(true),
            trip_update: Some(TripUpdate {
                trip: descriptor(),
                stop_time_update: vec![
                    trip_update::StopTimeUpdate {
                        stop_id: Some("stop".into()),
                        arrival: Some(gtfs_realtime::trip_update::StopTimeEvent {
                            delay: Some(60),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    trip_update::StopTimeUpdate {
                        stop_id: Some("missing".into()),
                        arrival: Some(gtfs_realtime::trip_update::StopTimeEvent {
                            delay: Some(60),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }),
            vehicle: Some(VehiclePosition {
                trip: Some(descriptor()),
                position: Some(Position {
                    latitude: 42.0,
                    longitude: -71.0,
                    ..Default::default()
                }),
                stop_id: Some("stop".into()),
                ..Default::default()
            }),
            alert: Some(alert),
            ..Default::default()
        };
        RtBatch {
            trip_updates: FeedMessage {
                entity: vec![mixed],
                ..Default::default()
            },
            vehicle_positions: FeedMessage::default(),
            alerts: FeedMessage::default(),
        }
    }

    #[tokio::test]
    async fn picks_first_non_empty_source() {
        let available = sources(vec![
            ("a", Behavior::Ok(batch_with(3))),
            ("b", Behavior::Ok(batch_with(1))),
        ]);
        let selected = select_batch(&available, &Gtfs::default()).await.unwrap();
        assert_eq!(selected.source_name, "a");
    }

    #[tokio::test]
    async fn skips_failures_and_empty_batches() {
        let available = sources(vec![
            ("a", Behavior::Fail),
            ("b", Behavior::Empty),
            ("c", Behavior::Ok(batch_with(1))),
        ]);
        assert_eq!(
            select_batch(&available, &Gtfs::default())
                .await
                .unwrap()
                .source_name,
            "c"
        );
    }

    #[test]
    fn builder_partitions_mixed_payloads_and_validator_round_trips() {
        let generated_at = UNIX_EPOCH + std::time::Duration::from_secs(1234);
        let candidate = GenerationBuilder::build(
            static_snapshot(),
            SelectedBatch {
                source_name: "fixture",
                batch: mixed_batch(),
            },
            generated_at,
        )
        .unwrap();
        for message in [
            &candidate.trip_updates,
            &candidate.vehicle_positions,
            &candidate.alerts,
        ] {
            assert_eq!(message.entity.len(), 1);
            assert_eq!(message.header.gtfs_realtime_version, "2.0");
            assert_eq!(message.header.timestamp, Some(1234));
            assert_eq!(message.header.feed_version.as_deref(), Some("STATIC-V1"));
            assert_eq!(message.entity[0].is_deleted, None);
        }
        assert!(candidate.trip_updates.entity[0].trip_update.is_some());
        assert_eq!(
            candidate.trip_updates.entity[0]
                .trip_update
                .as_ref()
                .unwrap()
                .stop_time_update
                .len(),
            1
        );
        assert!(candidate.vehicle_positions.entity[0].vehicle.is_some());
        assert!(candidate.alerts.entity[0].alert.is_some());

        let validated = CandidateValidator::validate(candidate).unwrap();
        assert_eq!(validated.entity_counts.trip_updates, 1);
        assert_eq!(validated.entity_counts.vehicle_positions, 1);
        assert_eq!(validated.entity_counts.alerts, 1);
        FeedMessage::decode(validated.trip_updates.as_ref()).unwrap();
        FeedMessage::decode(validated.vehicle_positions.as_ref()).unwrap();
        FeedMessage::decode(validated.alerts.as_ref()).unwrap();
        let manifest = validated.manifest(
            GenerationId("generation-1".into()),
            ArtifactUrls {
                static_zip: "/static.zip".into(),
                trip_updates: "/trip-updates.pb".into(),
                vehicle_positions: "/vehicle-positions.pb".into(),
                alerts: "/alerts.pb".into(),
            },
        );
        assert_eq!(manifest.generated_at_unix, 1234);
        assert_eq!(manifest.static_version, "STATIC-V1");
        assert_eq!(manifest.source, "fixture");
        assert_eq!(manifest.entity_counts, validated.entity_counts);
    }

    #[test]
    fn builder_deduplicates_unified_entities_repeated_across_source_products() {
        let mut batch = mixed_batch();
        batch.vehicle_positions = batch.trip_updates.clone();
        batch.alerts = batch.trip_updates.clone();
        let candidate = GenerationBuilder::build(
            static_snapshot(),
            SelectedBatch {
                source_name: "fixture",
                batch,
            },
            UNIX_EPOCH + Duration::from_secs(100),
        )
        .unwrap();
        assert_eq!(candidate.trip_updates.entity.len(), 1);
        assert_eq!(candidate.vehicle_positions.entity.len(), 1);
        assert_eq!(candidate.alerts.entity.len(), 1);
        CandidateValidator::validate(candidate).unwrap();
    }

    #[test]
    fn builder_omits_unmatched_invalid_geometry_and_empty_alerts() {
        let mut batch = mixed_batch();
        let entity = &mut batch.trip_updates.entity[0];
        entity.trip_update.as_mut().unwrap().trip.trip_id = Some("missing".into());
        entity
            .vehicle
            .as_mut()
            .unwrap()
            .position
            .as_mut()
            .unwrap()
            .latitude = f32::NAN;
        entity.alert.as_mut().unwrap().header_text = None;
        let candidate = GenerationBuilder::build(
            static_snapshot(),
            SelectedBatch {
                source_name: "fixture",
                batch,
            },
            UNIX_EPOCH,
        )
        .unwrap();
        assert!(candidate.trip_updates.entity.is_empty());
        assert!(candidate.vehicle_positions.entity.is_empty());
        assert!(candidate.alerts.entity.is_empty());
    }

    #[test]
    fn stop_predictions_resolve_by_id_sequence_or_consistent_both() {
        let snapshot = static_snapshot();
        let trip = descriptor();
        let event = Some(gtfs_realtime::trip_update::StopTimeEvent {
            delay: Some(60),
            ..Default::default()
        });
        let cases = [
            (
                trip_update::StopTimeUpdate {
                    stop_id: Some("stop".into()),
                    arrival: event,
                    ..Default::default()
                },
                true,
            ),
            (
                trip_update::StopTimeUpdate {
                    stop_sequence: Some(1),
                    arrival: event,
                    ..Default::default()
                },
                true,
            ),
            (
                trip_update::StopTimeUpdate {
                    stop_sequence: Some(1),
                    stop_id: Some("stop".into()),
                    arrival: event,
                    ..Default::default()
                },
                true,
            ),
            (
                trip_update::StopTimeUpdate {
                    stop_sequence: Some(2),
                    stop_id: Some("stop".into()),
                    arrival: event,
                    ..Default::default()
                },
                false,
            ),
            (
                trip_update::StopTimeUpdate {
                    stop_sequence: Some(1),
                    ..Default::default()
                },
                false,
            ),
        ];
        for (update, expected) in cases {
            assert_eq!(
                valid_stop_time_update(&update, &trip, &snapshot.parsed),
                expected
            );
        }

        let mut repeated_gtfs = Gtfs::default();
        let stop = snapshot.parsed.stops["stop"].clone();
        repeated_gtfs.stops.insert("stop".into(), stop.clone());
        let first = RawStopTime {
            stop_sequence: 1,
            ..Default::default()
        };
        let second = RawStopTime {
            stop_sequence: 2,
            ..Default::default()
        };
        repeated_gtfs.trips.insert(
            "trip".into(),
            Trip {
                id: "trip".into(),
                route_id: "route".into(),
                stop_times: vec![
                    StopTime::from(first, stop.clone()),
                    StopTime::from(second, stop),
                ],
                ..Default::default()
            },
        );
        assert!(!valid_stop_time_update(
            &trip_update::StopTimeUpdate {
                stop_id: Some("stop".into()),
                arrival: event,
                ..Default::default()
            },
            &trip,
            &repeated_gtfs,
        ));
    }

    #[test]
    fn validator_rejects_duplicate_ids_bad_headers_and_mixed_payloads() {
        let snapshot = static_snapshot();
        let generated_at = UNIX_EPOCH + std::time::Duration::from_secs(1234);
        let make_candidate = || {
            GenerationBuilder::build(
                snapshot.clone(),
                SelectedBatch {
                    source_name: "fixture",
                    batch: mixed_batch(),
                },
                generated_at,
            )
            .unwrap()
        };

        let mut duplicate = make_candidate();
        duplicate
            .trip_updates
            .entity
            .push(duplicate.trip_updates.entity[0].clone());
        assert!(CandidateValidator::validate(duplicate).is_err());

        let mut bad_header = make_candidate();
        bad_header.vehicle_positions.header.timestamp = Some(999);
        assert!(CandidateValidator::validate(bad_header).is_err());

        let mut mixed = make_candidate();
        mixed.alerts.entity[0].vehicle = Some(VehiclePosition::default());
        assert!(CandidateValidator::validate(mixed).is_err());

        let mut deletion = make_candidate();
        deletion.alerts.entity[0].is_deleted = Some(false);
        assert!(CandidateValidator::validate(deletion).is_err());
    }

    #[test]
    fn alert_route_type_must_be_valid_and_match_static_route() {
        let snapshot = static_snapshot();
        let valid = EntitySelector {
            route_id: Some("route".into()),
            route_type: Some(3),
            ..Default::default()
        };
        assert!(valid_alert_selector(&valid, &snapshot.parsed));
        assert!(!valid_alert_selector(
            &EntitySelector {
                route_type: Some(-1),
                ..Default::default()
            },
            &snapshot.parsed
        ));
        assert!(!valid_alert_selector(
            &EntitySelector {
                route_id: Some("route".into()),
                route_type: Some(2),
                ..Default::default()
            },
            &snapshot.parsed
        ));
        assert!(valid_alert_selector(
            &EntitySelector {
                route_type: Some(3),
                ..Default::default()
            },
            &snapshot.parsed
        ));
        assert!(!valid_alert_selector(
            &EntitySelector {
                route_type: Some(2),
                ..Default::default()
            },
            &snapshot.parsed
        ));
        assert!(!valid_alert_selector(
            &EntitySelector {
                route_type: Some(100),
                ..Default::default()
            },
            &snapshot.parsed
        ));
        assert!(valid_alert_selector(
            &EntitySelector {
                route_id: Some("route".into()),
                agency_id: Some("agency-a".into()),
                ..Default::default()
            },
            &snapshot.parsed
        ));
        assert!(!valid_alert_selector(
            &EntitySelector {
                trip: Some(descriptor()),
                agency_id: Some("agency-b".into()),
                ..Default::default()
            },
            &snapshot.parsed
        ));
    }

    #[test]
    fn assigned_stops_require_static_closure_sequence_and_no_data_without_predictions() {
        use gtfs_realtime::trip_update::stop_time_update::{
            ScheduleRelationship, StopTimeProperties,
        };

        let snapshot = static_snapshot();
        let trip = descriptor();
        let assignment = |assigned_stop_id: &str| StopTimeProperties {
            assigned_stop_id: Some(assigned_stop_id.into()),
            ..Default::default()
        };
        let valid = trip_update::StopTimeUpdate {
            stop_sequence: Some(1),
            schedule_relationship: Some(ScheduleRelationship::NoData as i32),
            stop_time_properties: Some(assignment("platform")),
            ..Default::default()
        };
        assert!(valid_stop_time_update(&valid, &trip, &snapshot.parsed));

        let mut missing_sequence = valid.clone();
        missing_sequence.stop_sequence = None;
        assert!(!valid_stop_time_update(
            &missing_sequence,
            &trip,
            &snapshot.parsed
        ));

        let mut missing_static_stop = valid.clone();
        missing_static_stop.stop_time_properties = Some(assignment("missing"));
        assert!(!valid_stop_time_update(
            &missing_static_stop,
            &trip,
            &snapshot.parsed
        ));

        let mut conflicting_stop_id = valid.clone();
        conflicting_stop_id.stop_id = Some("stop".into());
        assert!(!valid_stop_time_update(
            &conflicting_stop_id,
            &trip,
            &snapshot.parsed
        ));

        let mut wrong_relationship = valid;
        wrong_relationship.schedule_relationship = Some(ScheduleRelationship::Skipped as i32);
        assert!(!valid_stop_time_update(
            &wrong_relationship,
            &trip,
            &snapshot.parsed
        ));
    }

    struct RejectCommitter;

    #[async_trait]
    impl GenerationCommitter for RejectCommitter {
        async fn publish(
            &self,
            _candidate: ValidatedGeneration,
        ) -> Result<Arc<PublishedGeneration>, PublishError> {
            Err(PublishError("injected publication failure".into()))
        }
    }

    struct RejectProcessor(RefreshStage);

    impl GenerationProcessor for RejectProcessor {
        fn build_and_validate(
            &self,
            _snapshot: Arc<StaticSnapshot>,
            _selected: SelectedBatch,
            _generated_at: SystemTime,
        ) -> Result<ValidatedGeneration, RefreshStage> {
            Err(self.0)
        }
    }

    fn pending_snapshot(active: &Arc<StaticSnapshot>) -> Arc<StaticSnapshot> {
        Arc::new(StaticSnapshot {
            version: "STATIC-V2".into(),
            parsed: active.parsed.clone(),
            zip: Arc::from(&b"zip-v2"[..]),
        })
    }

    fn output_directory() -> PathBuf {
        std::env::temp_dir().join(format!(
            "amtrak-refresh-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[tokio::test]
    async fn pending_static_promotes_only_after_durable_publication_retry() {
        let active = static_snapshot();
        let pending = pending_snapshot(&active);
        let snapshots = StaticSnapshotState::new(active.clone());
        snapshots.stage(pending.clone()).await;
        let sources = sources(vec![("mock", Behavior::Ok(mixed_batch()))]);

        let failed = refresh_generation_once(
            &sources,
            &snapshots,
            &RejectCommitter,
            UNIX_EPOCH + Duration::from_secs(100),
        )
        .await;
        assert_eq!(failed.outcome, "failure");
        assert_eq!(failed.stage, RefreshStage::Publish);
        assert!(Arc::ptr_eq(&snapshots.active().await, &active));
        assert!(Arc::ptr_eq(&snapshots.pending().await.unwrap(), &pending));

        let output = output_directory();
        let store = GenerationStore::open(&output).await.unwrap();
        let committer = StoreGenerationCommitter::new(output.clone(), store.clone());
        let succeeded = refresh_generation_once(
            &sources,
            &snapshots,
            &committer,
            UNIX_EPOCH + Duration::from_secs(101),
        )
        .await;
        assert_eq!(succeeded.outcome, "success");
        assert_eq!(succeeded.stage, RefreshStage::Commit);
        assert_eq!(succeeded.source, Some("mock"));
        assert_eq!(succeeded.entity_counts.trip_updates, 1);
        assert_eq!(succeeded.entity_counts.vehicle_positions, 1);
        assert_eq!(succeeded.entity_counts.alerts, 1);
        assert!(Arc::ptr_eq(&snapshots.active().await, &pending));
        assert!(snapshots.pending().await.is_none());
        let current = store.current().await.unwrap();
        assert_eq!(current.manifest.static_version, "STATIC-V2");
        assert_eq!(current.manifest.generated_at_unix, 101);
        assert_eq!(Some(current.id.clone()), succeeded.generation_id);
        std::fs::remove_dir_all(output).unwrap();
    }

    #[tokio::test]
    async fn every_prepublication_failure_preserves_pending_static() {
        for (behavior, expected) in [
            (Behavior::Fail, RefreshStage::Source),
            (Behavior::Ok(batch_with(1)), RefreshStage::EmptyCandidate),
        ] {
            let active = static_snapshot();
            let pending = pending_snapshot(&active);
            let snapshots = StaticSnapshotState::new(active.clone());
            snapshots.stage(pending.clone()).await;
            let sources = sources(vec![("mock", behavior)]);
            let telemetry = refresh_generation_once(
                &sources,
                &snapshots,
                &RejectCommitter,
                UNIX_EPOCH + Duration::from_secs(100),
            )
            .await;
            assert_eq!(telemetry.stage, expected);
            assert_eq!(telemetry.outcome, "failure");
            assert!(Arc::ptr_eq(&snapshots.active().await, &active));
            assert!(Arc::ptr_eq(&snapshots.pending().await.unwrap(), &pending));
        }

        for stage in [RefreshStage::Build, RefreshStage::Validate] {
            let active = static_snapshot();
            let pending = pending_snapshot(&active);
            let snapshots = StaticSnapshotState::new(active.clone());
            snapshots.stage(pending.clone()).await;
            let sources = sources(vec![("mock", Behavior::Ok(mixed_batch()))]);
            let telemetry = refresh_generation_once_with(
                &sources,
                &snapshots,
                &RejectCommitter,
                &RejectProcessor(stage),
                UNIX_EPOCH + Duration::from_secs(100),
            )
            .await;
            assert_eq!(telemetry.stage, stage);
            assert_eq!(telemetry.outcome, "failure");
            assert!(Arc::ptr_eq(&snapshots.active().await, &active));
            assert!(Arc::ptr_eq(&snapshots.pending().await.unwrap(), &pending));
        }
    }

    #[test]
    fn refresh_telemetry_has_only_the_documented_allowlisted_fields() {
        let telemetry = RefreshTelemetry {
            outcome: "failure",
            stage: RefreshStage::Validate,
            source: Some("asm"),
            generation_id: None,
            static_version: "STATIC-V1".into(),
            duration_ms: 12,
            entity_counts: EntityCounts::default(),
        };
        let debug = format!("{telemetry:?}");
        for field in [
            "outcome",
            "stage",
            "source",
            "generation_id",
            "static_version",
            "duration_ms",
            "entity_counts",
        ] {
            assert!(debug.contains(field));
        }
        for forbidden in ["error", "url", "header", "credential", "config"] {
            assert!(!debug.contains(forbidden));
        }
    }

    #[tokio::test]
    #[ignore = "requires current Amtrak static and realtime endpoints"]
    async fn live_candidate_builds_and_passes_semantic_validation() {
        use crate::sources::amtrak::AmtrakSource;
        use std::io::Cursor;

        let response = reqwest::get("https://content.amtrak.com/content/gtfs/GTFS.zip")
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let zip: Arc<[u8]> = Arc::from(response.as_ref());
        let parsed = Gtfs::from_reader(Cursor::new(zip.clone())).unwrap();
        let version = parsed
            .feed_info
            .iter()
            .filter_map(|info| info.version.clone())
            .find(|value| !value.trim().is_empty())
            .unwrap();
        let snapshot = Arc::new(StaticSnapshot {
            version,
            parsed: Arc::new(parsed),
            zip,
        });
        let sources: Vec<Box<dyn RtSource>> = vec![Box::new(AmtrakSource::new())];
        let selected = select_batch(&sources, &snapshot.parsed).await.unwrap();
        let candidate = GenerationBuilder::build(snapshot, selected, SystemTime::now()).unwrap();
        CandidateValidator::validate(candidate)
            .unwrap_or_else(|error| panic!("live semantic validation failed: {}", error.0));
    }
}
