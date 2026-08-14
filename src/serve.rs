use crate::config::{authorize, AccessDecision, AccessPolicy};
use crate::orchestrator::{ArtifactUrls, FeedSetManifest, GenerationId};
use crate::writer::{GenerationStore, PublishedGeneration};
use axum::{
    extract::{ConnectInfo, Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Injectable wall clock used to make freshness boundaries deterministic.
pub trait Clock: Send + Sync {
    /// Returns the current wall-clock instant used only for readiness age.
    fn now(&self) -> SystemTime;
}

/// Production wall clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Shared HTTP dependencies for immutable generation delivery.
///
/// The store is the only artifact authority; route parameters are parsed into
/// closed identifiers and never joined to filesystem paths. Peer identity comes
/// only from Axum connection metadata, and readiness uses an injected clock so
/// the exact freshness boundary is observable and testable.
#[derive(Clone)]
pub struct AppState {
    pub store: GenerationStore,
    pub access_policy: AccessPolicy,
    pub freshness_limit: Duration,
    pub clock: Arc<dyn Clock>,
}

impl AppState {
    /// Creates HTTP state for one recovered generation store and access policy.
    ///
    /// # Arguments
    ///
    /// * `store` - Recovered durable generation authority shared by all handlers.
    /// * `access_policy` - Exact direct-peer policy for protected routes.
    /// * `freshness_limit` - Maximum generation age; equality is not ready.
    /// * `clock` - Wall clock used to compute observable readiness age.
    ///
    /// # Returns
    ///
    /// A cloneable state value that keeps all handlers on the same store.
    pub fn new(
        store: GenerationStore,
        access_policy: AccessPolicy,
        freshness_limit: Duration,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            store,
            access_policy,
            freshness_limit,
            clock,
        }
    }
}

/// Builds the versioned immutable API and independently observable liveness.
///
/// Protected handlers require `ConnectInfo<SocketAddr>` and fail closed when it
/// is absent. `Forwarded` and `X-Forwarded-For` are never read, because this
/// increment authorizes the direct transport peer only.
///
/// # Arguments
///
/// * `state` - Shared store, access policy, freshness limit, and clock.
///
/// # Returns
///
/// A router containing only the versioned immutable API and `/livez`.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/livez", get(livez))
        .route("/readyz", get(readyz))
        .route("/v1/feed-set.json", get(feed_set))
        .route("/v1/generations/{id}/{artifact}", get(artifact))
        .with_state(state)
}

/// Reports process liveness independently from feed availability or access.
///
/// # Returns
///
/// HTTP `200` with a JSON `live: true` body.
pub async fn livez() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "live": true })))
}

type Peer = Result<ConnectInfo<SocketAddr>, axum::extract::rejection::ExtensionRejection>;

/// Reports freshness of the latest durable generation for an authorized peer.
///
/// A generation is ready only while age is strictly less than the configured
/// limit; a future generation timestamp is treated as zero age rather than
/// wrapping. Before first publication the response is `503 no_generation`.
///
/// # Arguments
///
/// * `state` - Shared HTTP state containing the current generation and clock.
/// * `peer` - Direct transport-peer metadata or its extraction failure.
///
/// # Returns
///
/// `403` for a denied/missing peer, otherwise JSON `200` or `503` with age and
/// latest-success fields.
pub async fn readyz(State(state): State<AppState>, peer: Peer) -> Response {
    if let Some(response) = denied_peer(&state.access_policy, peer, "/readyz") {
        return response;
    }
    let Some(generation) = state.store.current().await else {
        return readiness_response(StatusCode::SERVICE_UNAVAILABLE, None, None, "no_generation");
    };
    let now = state
        .clock
        .now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let generated = generation.manifest.generated_at_unix;
    let age = now.saturating_sub(generated);
    let ready = age < state.freshness_limit.as_secs();
    readiness_response(
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Some(&generation),
        Some(age),
        if ready { "fresh" } else { "stale" },
    )
}

/// Returns discovery metadata for the one current immutable generation.
///
/// The manifest is reconstructed as JSON from the committed typed value; URLs,
/// counts, timestamp, and static version therefore refer to the same generation.
///
/// # Arguments
///
/// * `state` - Shared HTTP state containing the current immutable generation.
/// * `peer` - Direct transport-peer metadata or its extraction failure.
///
/// # Returns
///
/// `403` for a denied/missing peer, `503` before publication, or a coherent
/// manifest with HTTP `200`.
pub async fn feed_set(State(state): State<AppState>, peer: Peer) -> Response {
    if let Some(response) = denied_peer(&state.access_policy, peer, "/v1/feed-set.json") {
        return response;
    }
    match state.store.current().await {
        Some(generation) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            Json(manifest_json(&generation.manifest)),
        )
            .into_response(),
        None => (StatusCode::SERVICE_UNAVAILABLE, "feed unavailable").into_response(),
    }
}

/// Serves one closed artifact name from one strict immutable generation ID.
///
/// Malformed IDs and artifact names return `404`; valid but unknown generation
/// IDs also return `404` and never fall back to current. No route value becomes
/// a path component.
///
/// # Arguments
///
/// * `state` - Shared HTTP state used for store-only immutable lookup.
/// * `peer` - Direct transport-peer metadata or its extraction failure.
/// * `id` - Strict generation lookup key from the route.
/// * `artifact` - Closed artifact filename from the route.
///
/// # Returns
///
/// `403` for a denied/missing peer, `503` before first publication, `404` for a
/// malformed/unknown lookup, or the immutable bytes with HTTP `200`.
pub async fn artifact(
    State(state): State<AppState>,
    peer: Peer,
    Path((id, artifact)): Path<(String, String)>,
) -> Response {
    if let Some(response) = denied_peer(
        &state.access_policy,
        peer,
        "/v1/generations/{id}/{artifact}",
    ) {
        return response;
    }
    let Ok(id) = GenerationId::from_str(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(artifact) = Artifact::from_str(&artifact) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if state.store.current().await.is_none() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let Some(generation) = state.store.get(&id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (content_type, bytes) = artifact.bytes(&generation);
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, content_type)],
        bytes.as_ref().to_vec(),
    )
        .into_response()
}

fn denied_peer(policy: &AccessPolicy, peer: Peer, route: &'static str) -> Option<Response> {
    let Ok(ConnectInfo(peer)) = peer else {
        tracing::warn!(peer = "missing", route, outcome = "denied", "access audit");
        return Some(StatusCode::FORBIDDEN.into_response());
    };
    match authorize(policy, peer.ip()) {
        AccessDecision::Allow => None,
        AccessDecision::Deny => {
            tracing::warn!(peer = %peer.ip(), route, outcome = "denied", "access audit");
            Some(StatusCode::FORBIDDEN.into_response())
        }
    }
}

fn readiness_response(
    status: StatusCode,
    generation: Option<&PublishedGeneration>,
    age_seconds: Option<u64>,
    reason: &'static str,
) -> Response {
    let value = json!({
        "ready": status == StatusCode::OK,
        "reason": reason,
        "generation_id": generation.map(|value| value.id.0.as_str()),
        "latest_success_unix": generation.map(|value| value.manifest.generated_at_unix),
        "age_seconds": age_seconds,
    });
    (status, Json(value)).into_response()
}

fn manifest_json(manifest: &FeedSetManifest) -> Value {
    let ArtifactUrls {
        static_zip,
        trip_updates,
        vehicle_positions,
        alerts,
    } = &manifest.urls;
    json!({
        "generation_id": manifest.generation_id.0,
        "generated_at_unix": manifest.generated_at_unix,
        "static_version": manifest.static_version,
        "source": manifest.source,
        "entity_counts": {
            "trip_updates": manifest.entity_counts.trip_updates,
            "vehicle_positions": manifest.entity_counts.vehicle_positions,
            "alerts": manifest.entity_counts.alerts,
        },
        "urls": {
            "static_zip": static_zip,
            "trip_updates": trip_updates,
            "vehicle_positions": vehicle_positions,
            "alerts": alerts,
        }
    })
}

#[derive(Clone, Copy)]
enum Artifact {
    Static,
    TripUpdates,
    VehiclePositions,
    Alerts,
}

impl FromStr for Artifact {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "static.zip" => Ok(Self::Static),
            "trip-updates.pb" => Ok(Self::TripUpdates),
            "vehicle-positions.pb" => Ok(Self::VehiclePositions),
            "alerts.pb" => Ok(Self::Alerts),
            _ => Err(()),
        }
    }
}

impl Artifact {
    fn bytes(self, generation: &PublishedGeneration) -> (&'static str, Arc<[u8]>) {
        match self {
            Self::Static => ("application/zip", generation.static_zip.clone()),
            Self::TripUpdates => ("application/x-protobuf", generation.trip_updates.clone()),
            Self::VehiclePositions => (
                "application/x-protobuf",
                generation.vehicle_positions.clone(),
            ),
            Self::Alerts => ("application/x-protobuf", generation.alerts.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::{
        CandidateValidator, GenerationBuilder, SelectedBatch, StaticSnapshot,
    };
    use crate::sources::RtBatch;
    use crate::writer::GenerationPublisher;
    use axum::{
        body::{to_bytes, Body},
        extract::connect_info::MockConnectInfo,
        http::Request,
    };
    use gtfs_realtime::{FeedEntity, FeedMessage};
    use gtfs_structures::Gtfs;
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use tower::ServiceExt;

    struct FixedClock(SystemTime);

    impl Clock for FixedClock {
        fn now(&self) -> SystemTime {
            self.0
        }
    }

    fn output_directory(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "amtrak-serve-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    async fn fixture_state(
        label: &str,
        now: u64,
        published_at: Option<u64>,
    ) -> (AppState, PathBuf) {
        let output = output_directory(label);
        let store = GenerationStore::open(&output).await.unwrap();
        if let Some(generated_at) = published_at {
            let mut trip_updates = FeedMessage::default();
            trip_updates.entity.push(FeedEntity {
                id: "trip".into(),
                ..Default::default()
            });
            let candidate = GenerationBuilder::build(
                Arc::new(StaticSnapshot {
                    version: "STATIC-V1".into(),
                    parsed: Arc::new(Gtfs::default()),
                    zip: Arc::from(&b"PK fixture"[..]),
                }),
                SelectedBatch {
                    source_name: "fixture",
                    batch: RtBatch {
                        trip_updates,
                        vehicle_positions: FeedMessage::default(),
                        alerts: FeedMessage::default(),
                    },
                },
                UNIX_EPOCH + Duration::from_secs(generated_at),
            )
            .unwrap();
            let validated = CandidateValidator::validate(candidate).unwrap();
            GenerationPublisher::publish(&output, &store, validated)
                .await
                .unwrap();
        }
        let policy = AccessPolicy::from_allowed_ips_for_test(BTreeSet::new());
        (
            AppState::new(
                store,
                policy,
                Duration::from_secs(300),
                Arc::new(FixedClock(UNIX_EPOCH + Duration::from_secs(now))),
            ),
            output,
        )
    }

    fn request(path: &str) -> Request<Body> {
        Request::builder().uri(path).body(Body::empty()).unwrap()
    }

    async fn body_json(response: Response) -> Value {
        serde_json::from_slice(&to_bytes(response.into_body(), 1_000_000).await.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn liveness_is_public_but_protected_routes_fail_closed_without_peer() {
        let (state, output) = fixture_state("missing-peer", 100, Some(100)).await;
        let app = router(state);
        assert_eq!(
            app.clone()
                .oneshot(request("/livez"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        for path in [
            "/readyz",
            "/v1/feed-set.json",
            "/v1/generations/1-0/static.zip",
        ] {
            assert_eq!(
                app.clone().oneshot(request(path)).await.unwrap().status(),
                StatusCode::FORBIDDEN
            );
        }
        std::fs::remove_dir_all(output).unwrap();
    }

    #[tokio::test]
    async fn authorization_uses_only_direct_peer_and_ignores_forwarding_headers() {
        let (state, output) = fixture_state("auth", 100, Some(100)).await;
        let denied =
            router(state.clone()).layer(MockConnectInfo(SocketAddr::from(([10, 0, 0, 1], 7))));
        let spoofed = Request::builder()
            .uri("/v1/feed-set.json")
            .header("Forwarded", "for=127.0.0.1")
            .header("X-Forwarded-For", "127.0.0.1")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            denied.oneshot(spoofed).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );

        let allowed =
            router(state.clone()).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 8))));
        assert_eq!(
            allowed
                .oneshot(request("/v1/feed-set.json"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );

        let mut allowed_ips = BTreeSet::new();
        allowed_ips.insert("10.0.0.1".parse().unwrap());
        let explicit = AppState::new(
            state.store,
            AccessPolicy::from_allowed_ips_for_test(allowed_ips),
            state.freshness_limit,
            state.clock,
        );
        let explicitly_allowed =
            router(explicit).layer(MockConnectInfo(SocketAddr::from(([10, 0, 0, 1], 12))));
        assert_eq!(
            explicitly_allowed
                .oneshot(request("/v1/feed-set.json"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        std::fs::remove_dir_all(output).unwrap();
    }

    #[tokio::test]
    async fn readiness_obeys_no_generation_and_exact_freshness_boundaries() {
        for (label, now, published, expected, reason, age) in [
            (
                "none",
                100,
                None,
                StatusCode::SERVICE_UNAVAILABLE,
                "no_generation",
                None,
            ),
            ("young", 399, Some(100), StatusCode::OK, "fresh", Some(299)),
            (
                "exact",
                400,
                Some(100),
                StatusCode::SERVICE_UNAVAILABLE,
                "stale",
                Some(300),
            ),
            (
                "stale",
                401,
                Some(100),
                StatusCode::SERVICE_UNAVAILABLE,
                "stale",
                Some(301),
            ),
            ("future", 99, Some(100), StatusCode::OK, "fresh", Some(0)),
        ] {
            let (state, output) = fixture_state(label, now, published).await;
            let app = router(state).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 9))));
            let response = app.oneshot(request("/readyz")).await.unwrap();
            assert_eq!(response.status(), expected);
            let body = body_json(response).await;
            assert_eq!(body["reason"], reason);
            assert_eq!(body["age_seconds"], age.map_or(Value::Null, Value::from));
            std::fs::remove_dir_all(output).unwrap();
        }
    }

    #[tokio::test]
    async fn a_new_commit_recovers_readiness_after_staleness() {
        let (state, output) = fixture_state("readiness-recovery", 400, Some(100)).await;
        let app =
            router(state.clone()).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 14))));
        assert_eq!(
            app.clone()
                .oneshot(request("/readyz"))
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );

        let candidate = GenerationBuilder::build(
            Arc::new(StaticSnapshot {
                version: "STATIC-V2".into(),
                parsed: Arc::new(Gtfs::default()),
                zip: Arc::from(&b"PK fixture 2"[..]),
            }),
            SelectedBatch {
                source_name: "fixture",
                batch: RtBatch {
                    trip_updates: FeedMessage::default(),
                    vehicle_positions: FeedMessage::default(),
                    alerts: FeedMessage::default(),
                },
            },
            UNIX_EPOCH + Duration::from_secs(200),
        )
        .unwrap();
        GenerationPublisher::publish(
            &output,
            &state.store,
            CandidateValidator::validate(candidate).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(
            app.oneshot(request("/readyz")).await.unwrap().status(),
            StatusCode::OK
        );
        std::fs::remove_dir_all(output).unwrap();
    }

    #[tokio::test]
    async fn manifest_and_every_artifact_are_generation_pinned_and_typed() {
        let (state, output) = fixture_state("manifest", 100, Some(100)).await;
        let app = router(state).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 10))));
        let manifest_response = app
            .clone()
            .oneshot(request("/v1/feed-set.json"))
            .await
            .unwrap();
        assert_eq!(manifest_response.status(), StatusCode::OK);
        assert_eq!(
            manifest_response.headers()[header::CONTENT_TYPE],
            "application/json"
        );
        let manifest = body_json(manifest_response).await;
        let id = manifest["generation_id"].as_str().unwrap();
        assert_eq!(manifest["static_version"], "STATIC-V1");
        assert_eq!(manifest["generated_at_unix"], 100);
        assert_eq!(manifest["source"], "fixture");
        assert_eq!(manifest["entity_counts"]["trip_updates"], 0);
        for (key, content_type, expected_prefix) in [
            ("static_zip", "application/zip", b"PK".as_slice()),
            ("trip_updates", "application/x-protobuf", b"\n".as_slice()),
            (
                "vehicle_positions",
                "application/x-protobuf",
                b"\n".as_slice(),
            ),
            ("alerts", "application/x-protobuf", b"\n".as_slice()),
        ] {
            let url = manifest["urls"][key].as_str().unwrap();
            assert!(url.starts_with(&format!("/v1/generations/{id}/")));
            let response = app.clone().oneshot(request(url)).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_TYPE], content_type);
            let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
            assert!(bytes.starts_with(expected_prefix));
        }
        std::fs::remove_dir_all(output).unwrap();
    }

    #[tokio::test]
    async fn closed_routes_reject_unknown_traversal_and_removed_legacy_paths() {
        let (state, output) = fixture_state("closed", 100, Some(100)).await;
        let app = router(state).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 11))));
        for path in [
            "/health",
            "/trip-updates.pb",
            "/static.zip",
            "/v1/generations/1-2/missing.pb",
            "/v1/generations/not-an-id/static.zip",
            "/v1/generations/1-2/%2e%2e",
            "/v1/generations/999-0/static.zip",
        ] {
            assert_eq!(
                app.clone().oneshot(request(path)).await.unwrap().status(),
                StatusCode::NOT_FOUND,
                "{path}"
            );
        }
        std::fs::remove_dir_all(output).unwrap();
    }

    #[tokio::test]
    async fn valid_artifact_request_is_unavailable_before_first_generation() {
        let (state, output) = fixture_state("artifact-unavailable", 100, None).await;
        let app = router(state).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 13))));
        assert_eq!(
            app.oneshot(request("/v1/generations/1-0/static.zip"))
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        std::fs::remove_dir_all(output).unwrap();
    }
}
