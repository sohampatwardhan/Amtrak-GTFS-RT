mod config;
mod orchestrator;
mod serve;
mod sources;
mod static_gtfs;
mod writer;

use crate::config::Config;
use crate::orchestrator::{GenerationCommitter, StoreGenerationCommitter};
use crate::sources::amtrak::AmtrakSource;
use crate::sources::{RtBatch, RtSource, SourceError};
use crate::static_gtfs::{
    bootstrap_static, recover_static, MobilityDataStaticValidator, StaticSnapshotState,
    StaticStandardsValidator,
};
use crate::writer::GenerationStore;
use async_trait::async_trait;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;

/// One long-lived activity owned by the service supervisor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceActivity {
    Poller,
    StaticRefresh,
    Http,
    Supervisor,
}

impl ServiceActivity {
    fn as_str(self) -> &'static str {
        match self {
            Self::Poller => "poller",
            Self::StaticRefresh => "static_refresh",
            Self::Http => "http",
            Self::Supervisor => "supervisor",
        }
    }
}

/// Closed, credential-free reason the service must exit non-zero.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceError {
    activity: ServiceActivity,
    outcome: &'static str,
}

impl ServiceError {
    fn stopped(activity: ServiceActivity) -> Self {
        Self {
            activity,
            outcome: "stopped",
        }
    }

    fn failed(activity: ServiceActivity) -> Self {
        Self {
            activity,
            outcome: "failed",
        }
    }
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} activity {} unexpectedly",
            self.activity.as_str(),
            self.outcome
        )
    }
}

impl std::error::Error for ServiceError {}

/// Runs all required activities as one failure domain.
///
/// Any success, error, or panic is unexpected. The supervisor aborts and awaits
/// every sibling before returning a closed error so the process exits non-zero
/// without leaking source, URL, header, or credential text.
pub async fn supervise<P, S, H>(poller: P, static_refresh: S, http: H) -> Result<(), ServiceError>
where
    P: Future<Output = Result<(), ServiceError>> + Send + 'static,
    S: Future<Output = Result<(), ServiceError>> + Send + 'static,
    H: Future<Output = Result<(), ServiceError>> + Send + 'static,
{
    let mut tasks = JoinSet::new();
    let mut activities = HashMap::new();
    activities.insert(tasks.spawn(poller).id(), ServiceActivity::Poller);
    activities.insert(
        tasks.spawn(static_refresh).id(),
        ServiceActivity::StaticRefresh,
    );
    activities.insert(tasks.spawn(http).id(), ServiceActivity::Http);

    let failure = match tasks.join_next_with_id().await {
        Some(Ok((id, Ok(())))) => ServiceError::stopped(
            activities
                .get(&id)
                .copied()
                .unwrap_or(ServiceActivity::Supervisor),
        ),
        Some(Ok((_id, Err(error)))) => error,
        Some(Err(error)) => ServiceError::failed(
            activities
                .get(&error.id())
                .copied()
                .unwrap_or(ServiceActivity::Supervisor),
        ),
        None => ServiceError::failed(ServiceActivity::Supervisor),
    };
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Err(failure)
}

struct CapitalCorridorFiltered<S>(S);

#[async_trait]
impl<S> RtSource for CapitalCorridorFiltered<S>
where
    S: RtSource + Send + Sync,
{
    fn name(&self) -> &'static str {
        self.0.name()
    }

    async fn fetch(&self, gtfs: &gtfs_structures::Gtfs) -> Result<RtBatch, SourceError> {
        let batch = self.0.fetch(gtfs).await?;
        Ok(RtBatch {
            trip_updates: amtrak_gtfs_rt::filter_capital_corridor(batch.trip_updates),
            vehicle_positions: amtrak_gtfs_rt::filter_capital_corridor(batch.vehicle_positions),
            alerts: amtrak_gtfs_rt::filter_capital_corridor(batch.alerts),
        })
    }
}

async fn initial_snapshots(
    store: &GenerationStore,
    static_url: &str,
    validator: &dyn StaticStandardsValidator,
) -> Result<StaticSnapshotState, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(generation) = store.current().await {
        return Ok(recover_static(
            generation.static_zip.clone(),
            generation.manifest.static_version.clone(),
        )?);
    }
    Ok(bootstrap_static(static_url, validator).await?)
}

fn container_healthcheck() -> std::io::Result<()> {
    let bind_address =
        std::env::var("AMTRAK_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
    let address = healthcheck_address(&bind_address)?;
    let timeout = Duration::from_secs(4);
    let mut stream = TcpStream::connect_timeout(&address, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(b"GET /livez HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;

    let mut response = [0_u8; 64];
    let bytes_read = stream.read(&mut response)?;
    if response[..bytes_read].starts_with(b"HTTP/1.1 200 ")
        || response[..bytes_read].starts_with(b"HTTP/1.0 200 ")
    {
        Ok(())
    } else {
        Err(std::io::Error::other("/livez did not return HTTP 200"))
    }
}

fn healthcheck_address(bind_address: &str) -> std::io::Result<SocketAddr> {
    let configured: SocketAddr = bind_address.parse().map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid AMTRAK_BIND_ADDR for healthcheck: {error}"),
        )
    })?;
    Ok(SocketAddr::from(([127, 0, 0, 1], configured.port())))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--healthcheck")) {
        container_healthcheck()?;
        return Ok(());
    }

    tracing_subscriber::fmt::init();

    // Validation and durable recovery finish before any task or listener exists.
    let config = Config::from_env()?.validate()?;
    let store = GenerationStore::open(&config.output_dir).await?;
    let validator: Arc<dyn StaticStandardsValidator> = Arc::new(MobilityDataStaticValidator::new(
        config.gtfs_validator_jar.clone(),
    ));
    let snapshots = initial_snapshots(&store, &config.static_url, validator.as_ref()).await?;

    let sources: Arc<Vec<Box<dyn RtSource>>> = if config.filter_capital_corridor {
        Arc::new(vec![Box::new(CapitalCorridorFiltered(AmtrakSource::new()))])
    } else {
        Arc::new(vec![Box::new(AmtrakSource::new())])
    };
    let committer: Arc<dyn GenerationCommitter> = Arc::new(StoreGenerationCommitter::new(
        config.output_dir.clone(),
        store.clone(),
    ));
    let app = serve::router(serve::AppState::new(
        store,
        config.access_policy,
        config.freshness_limit,
        Arc::new(serve::SystemClock),
    ));
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;

    let poller_snapshots = snapshots.clone();
    let refresh_snapshots = snapshots;
    let poll_interval = config.poll_interval;
    let static_url = config.static_url;
    let static_refresh_interval = config.static_refresh_interval;
    supervise(
        async move {
            orchestrator::run_poller(sources, poller_snapshots, committer, poll_interval).await;
            Ok(())
        },
        async move {
            static_gtfs::run_snapshot_refresh(
                refresh_snapshots,
                static_url,
                static_refresh_interval,
                validator,
            )
            .await;
            Ok(())
        },
        async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .map_err(|_| ServiceError::failed(ServiceActivity::Http))
        },
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::{EntityCounts, RefreshStage, StaticSnapshot, ValidatedGeneration};
    use crate::sources::mock::{Behavior, MockSource};
    use crate::static_gtfs::StaticValidationError;
    use crate::writer::GenerationPublisher;
    use gtfs_realtime::{FeedHeader, FeedMessage};
    use prost::Message;
    use std::io::{Cursor, Write};
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use zip::write::SimpleFileOptions;

    struct CancellationFlag(Arc<AtomicBool>);

    impl Drop for CancellationFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    type TestFuture = Pin<Box<dyn Future<Output = Result<(), ServiceError>> + Send + 'static>>;

    fn activity_future(
        selected: bool,
        result: Result<(), ServiceError>,
        dropped: Arc<AtomicBool>,
    ) -> TestFuture {
        let guard = CancellationFlag(dropped);
        if selected {
            Box::pin(async move {
                let _guard = guard;
                result
            })
        } else {
            Box::pin(async move {
                let _guard = guard;
                std::future::pending().await
            })
        }
    }

    fn output_directory(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "amtrak-composition-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn static_zip() -> Arc<[u8]> {
        let cursor = Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default();
        for (name, contents) in [
            ("agency.txt", "agency_id,agency_name,agency_url,agency_timezone\na,Amtrak,https://amtrak.com,America/New_York\n"),
            ("stops.txt", "stop_id,stop_name,stop_lat,stop_lon\ns,Station,42,-71\n"),
            ("routes.txt", "route_id,agency_id,route_short_name,route_long_name,route_type\nr,a,R,Regional,2\n"),
            ("trips.txt", "route_id,service_id,trip_id\nr,svc,t\n"),
            ("stop_times.txt", "trip_id,arrival_time,departure_time,stop_id,stop_sequence\nt,10:00:00,10:00:00,s,1\n"),
            ("calendar_dates.txt", "service_id,date,exception_type\nsvc,20260813,1\n"),
        ] {
            archive.start_file(name, options).unwrap();
            archive.write_all(contents.as_bytes()).unwrap();
        }
        Arc::from(archive.finish().unwrap().into_inner())
    }

    fn encoded_feed(timestamp: u64, version: &str) -> Arc<[u8]> {
        Arc::from(
            FeedMessage {
                header: FeedHeader {
                    gtfs_realtime_version: "2.0".into(),
                    incrementality: None,
                    timestamp: Some(timestamp),
                    feed_version: Some(version.into()),
                },
                entity: Vec::new(),
            }
            .encode_to_vec(),
        )
    }

    struct RejectIfCalled;

    #[async_trait]
    impl StaticStandardsValidator for RejectIfCalled {
        async fn validate(&self, _zip: Arc<[u8]>) -> Result<(), StaticValidationError> {
            panic!("recovery must not invoke the network static validator")
        }
    }

    #[tokio::test]
    async fn every_activity_success_and_failure_cancels_and_awaits_siblings() {
        for activity in [
            ServiceActivity::Poller,
            ServiceActivity::StaticRefresh,
            ServiceActivity::Http,
        ] {
            for fails in [false, true] {
                let dropped = [
                    Arc::new(AtomicBool::new(false)),
                    Arc::new(AtomicBool::new(false)),
                    Arc::new(AtomicBool::new(false)),
                ];
                let selected_result = if fails {
                    Err(ServiceError::failed(activity))
                } else {
                    Ok(())
                };
                let result = supervise(
                    activity_future(
                        activity == ServiceActivity::Poller,
                        selected_result.clone(),
                        dropped[0].clone(),
                    ),
                    activity_future(
                        activity == ServiceActivity::StaticRefresh,
                        selected_result.clone(),
                        dropped[1].clone(),
                    ),
                    activity_future(
                        activity == ServiceActivity::Http,
                        selected_result,
                        dropped[2].clone(),
                    ),
                )
                .await;
                let expected = if fails {
                    ServiceError::failed(activity)
                } else {
                    ServiceError::stopped(activity)
                };
                assert_eq!(result, Err(expected));
                assert!(dropped.iter().all(|flag| flag.load(Ordering::SeqCst)));
            }
        }
    }

    #[tokio::test]
    async fn panic_names_the_activity_and_cancels_siblings() {
        let refresh_cancelled = Arc::new(AtomicBool::new(false));
        let http_cancelled = Arc::new(AtomicBool::new(false));
        let result = supervise(
            async {
                panic!("fixture panic");
                #[allow(unreachable_code)]
                Ok(())
            },
            activity_future(false, Ok(()), refresh_cancelled.clone()),
            activity_future(false, Ok(()), http_cancelled.clone()),
        )
        .await;
        assert_eq!(result, Err(ServiceError::failed(ServiceActivity::Poller)));
        assert!(refresh_cancelled.load(Ordering::SeqCst));
        assert!(http_cancelled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn restart_recovers_last_good_when_refresh_source_is_unavailable() {
        let output = output_directory("restart");
        let store = GenerationStore::open(&output).await.unwrap();
        let recovered = recover_static(static_zip(), "retained-static".into()).unwrap();
        let snapshot: Arc<StaticSnapshot> = recovered.active().await;
        let generated_at = 1_786_588_800;
        let feed = encoded_feed(generated_at, &snapshot.version);
        let published = GenerationPublisher::publish(
            &output,
            &store,
            ValidatedGeneration {
                static_snapshot: snapshot,
                generated_at_unix: generated_at,
                source_name: "fixture",
                trip_updates: feed.clone(),
                vehicle_positions: feed.clone(),
                alerts: feed,
                entity_counts: EntityCounts::default(),
            },
        )
        .await
        .unwrap();
        let retained_id = published.id.clone();
        drop(store);

        let reopened = GenerationStore::open(&output).await.unwrap();
        let snapshots = initial_snapshots(
            &reopened,
            "http://127.0.0.1:1/unavailable.zip",
            &RejectIfCalled,
        )
        .await
        .unwrap();
        assert_eq!(snapshots.active().await.version, "retained-static");

        let sources: Vec<Box<dyn RtSource>> = vec![Box::new(MockSource {
            name: "offline",
            behavior: Behavior::Fail,
        })];
        let committer = StoreGenerationCommitter::new(output.clone(), reopened.clone());
        let outcome = orchestrator::refresh_generation_once(
            &sources,
            &snapshots,
            &committer,
            UNIX_EPOCH + Duration::from_secs(generated_at + 1),
        )
        .await;
        assert_eq!(outcome.stage, RefreshStage::Source);
        assert_eq!(reopened.current().await.unwrap().id, retained_id);

        std::fs::remove_dir_all(output).unwrap();
    }
}
#[test]
fn healthcheck_uses_configured_port_on_loopback() {
    assert_eq!(
        healthcheck_address("0.0.0.0:9000").unwrap(),
        "127.0.0.1:9000".parse::<SocketAddr>().unwrap()
    );
    assert_eq!(
        healthcheck_address("[::1]:7000").unwrap(),
        "127.0.0.1:7000".parse::<SocketAddr>().unwrap()
    );
    assert!(healthcheck_address("not-an-address").is_err());
}
