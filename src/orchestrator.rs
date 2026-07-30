use crate::sources::{RtBatch, RtSource};
use crate::static_gtfs::StaticFeedStore;
use gtfs_structures::Gtfs;
use prost::Message;
use std::path::Path;

/// Try each source in order; return the first successful, non-empty batch.
/// An empty or failing source is logged and skipped. `None` means no source had
/// fresh data this cycle — the caller then leaves the last-good files in place.
pub async fn select_batch(
    sources: &[Box<dyn RtSource>],
    gtfs: &Gtfs,
) -> Option<(&'static str, RtBatch)> {
    for source in sources {
        match source.fetch(gtfs).await {
            Ok(batch) if !batch.is_empty() => return Some((source.name(), batch)),
            Ok(_) => tracing::warn!(source = source.name(), "source returned empty batch"),
            Err(e) => tracing::warn!(source = source.name(), error = %e, "source fetch failed"),
        }
    }
    None
}

/// Encode the three feeds to protobuf and write them atomically. When
/// `filter_capital_corridor` is set, route 84 entities are dropped from each
/// feed (a better Capital Corridor feed is published elsewhere via 511.org).
/// Each feed's header is stamped with `feed_version` so consumers can confirm
/// the realtime feed matches the currently-served static feed.
pub fn write_feeds(
    dir: &Path,
    batch: RtBatch,
    filter_capital_corridor: bool,
    feed_version: &str,
) -> std::io::Result<()> {
    let (mut tu, mut vp, mut al) = if filter_capital_corridor {
        (
            amtrak_gtfs_rt::filter_capital_corridor(batch.trip_updates),
            amtrak_gtfs_rt::filter_capital_corridor(batch.vehicle_positions),
            amtrak_gtfs_rt::filter_capital_corridor(batch.alerts),
        )
    } else {
        (batch.trip_updates, batch.vehicle_positions, batch.alerts)
    };
    for msg in [&mut tu, &mut vp, &mut al] {
        msg.header.feed_version = Some(feed_version.to_string());
    }
    crate::writer::write_atomic(&dir.join("trip-updates.pb"), &tu.encode_to_vec())?;
    crate::writer::write_atomic(&dir.join("vehicle-positions.pb"), &vp.encode_to_vec())?;
    crate::writer::write_atomic(&dir.join("alerts.pb"), &al.encode_to_vec())?;
    Ok(())
}

/// The poll loop: every `poll_interval`, select a batch and write it. On a cycle
/// with no fresh data, the previous files remain (serving last-good).
pub async fn run_poller(
    sources: std::sync::Arc<Vec<Box<dyn RtSource>>>,
    store: StaticFeedStore,
    config: crate::config::Config,
) {
    let mut ticker = tokio::time::interval(config.poll_interval);
    loop {
        ticker.tick().await;
        let feed = store.get().await;
        match select_batch(sources.as_slice(), &feed.gtfs).await {
            Some((name, batch)) => {
                let (tu, vp, al) = (
                    batch.trip_updates.entity.len(),
                    batch.vehicle_positions.entity.len(),
                    batch.alerts.entity.len(),
                );
                match write_feeds(
                    &config.output_dir,
                    batch,
                    config.filter_capital_corridor,
                    &feed.feed_version,
                ) {
                    Ok(()) => tracing::info!(
                        source = name,
                        trip_updates = tu,
                        vehicles = vp,
                        alerts = al,
                        "wrote feeds"
                    ),
                    Err(e) => tracing::error!(error = %e, "failed to write feeds"),
                }
            }
            None => tracing::warn!("no fresh data from any source; serving last-good"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::mock::{batch_with, Behavior, MockSource};

    fn sources(behaviors: Vec<(&'static str, Behavior)>) -> Vec<Box<dyn RtSource>> {
        behaviors
            .into_iter()
            .map(|(name, behavior)| {
                Box::new(MockSource { name, behavior }) as Box<dyn RtSource>
            })
            .collect()
    }

    #[tokio::test]
    async fn picks_first_non_empty_source() {
        let s = sources(vec![
            ("a", Behavior::Ok(batch_with(3))),
            ("b", Behavior::Ok(batch_with(1))),
        ]);
        let (name, batch) = select_batch(&s, &Gtfs::default()).await.unwrap();
        assert_eq!(name, "a");
        assert_eq!(batch.trip_updates.entity.len(), 3);
    }

    #[tokio::test]
    async fn skips_empty_then_uses_next() {
        let s = sources(vec![
            ("a", Behavior::Empty),
            ("b", Behavior::Ok(batch_with(2))),
        ]);
        let (name, _) = select_batch(&s, &Gtfs::default()).await.unwrap();
        assert_eq!(name, "b");
    }

    #[tokio::test]
    async fn skips_failing_then_uses_next() {
        let s = sources(vec![
            ("a", Behavior::Fail),
            ("b", Behavior::Ok(batch_with(1))),
        ]);
        let (name, _) = select_batch(&s, &Gtfs::default()).await.unwrap();
        assert_eq!(name, "b");
    }

    #[tokio::test]
    async fn returns_none_when_all_fail_or_empty() {
        let s = sources(vec![("a", Behavior::Fail), ("b", Behavior::Empty)]);
        assert!(select_batch(&s, &Gtfs::default()).await.is_none());
    }

    #[test]
    fn write_feeds_writes_three_decodable_files() {
        let dir = std::env::temp_dir().join(format!("amtrak-orch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_feeds(&dir, batch_with(2), false, "TESTVER").unwrap();
        for name in ["trip-updates.pb", "vehicle-positions.pb", "alerts.pb"] {
            let bytes = std::fs::read(dir.join(name)).unwrap();
            // must round-trip through the protobuf decoder
            gtfs_realtime::FeedMessage::decode(bytes.as_slice()).unwrap();
        }
        let tu = gtfs_realtime::FeedMessage::decode(
            std::fs::read(dir.join("trip-updates.pb")).unwrap().as_slice(),
        )
        .unwrap();
        assert_eq!(tu.entity.len(), 2);
        // each feed's header is stamped with the active static feed_version
        assert_eq!(tu.header.feed_version.as_deref(), Some("TESTVER"));
    }
}
