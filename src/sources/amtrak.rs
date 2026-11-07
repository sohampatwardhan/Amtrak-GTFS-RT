use crate::sources::{RtBatch, RtSource, SourceError};
use async_trait::async_trait;
use gtfs_structures::Gtfs;

/// Primary realtime source: delegates to catenary's `amtrak-gtfs-rt` crate,
/// which fetches and decrypts Amtrak's `getTrainsData`, matches trains to GTFS
/// trips (handling multi-day date offsets), and returns ready-made FeedMessages.
pub struct AmtrakSource {
    client: reqwest::Client,
}

impl AmtrakSource {
    pub fn new() -> AmtrakSource {
        AmtrakSource {
            client: reqwest::Client::new(),
        }
    }
}

impl Default for AmtrakSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RtSource for AmtrakSource {
    fn name(&self) -> &'static str {
        "amtrak"
    }

    async fn fetch(&self, gtfs: &Gtfs) -> Result<RtBatch, SourceError> {
        let results = amtrak_gtfs_rt::fetch_amtrak_gtfs_rt(gtfs, &self.client).await?;
        Ok(RtBatch {
            trip_updates: results.trip_updates,
            vehicle_positions: results.vehicle_positions,
            alerts: results.alerts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_is_named_amtrak() {
        let src = AmtrakSource::new();
        assert_eq!(src.name(), "amtrak");
    }

    // Live test: hits Amtrak's real endpoints. Run explicitly with:
    //   cargo test sources::amtrak -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn live_fetch_returns_batch() {
        tokio::time::timeout(std::time::Duration::from_secs(180), async {
            let gtfs = Gtfs::from_url_async("https://content.amtrak.com/content/gtfs/GTFS.zip")
                .await
                .unwrap();
            let src = AmtrakSource::new();
            let batch = src.fetch(&gtfs).await.unwrap();
            // At virtually any hour some Amtrak trains are running.
            assert!(
                !batch.trip_updates.entity.is_empty(),
                "expected live trip updates"
            );
            assert!(
                !batch.vehicle_positions.entity.is_empty(),
                "expected live vehicle positions"
            );
            println!(
                "Amtrak source: trip_updates={}, vehicle_positions={}, alerts={}",
                batch.trip_updates.entity.len(),
                batch.vehicle_positions.entity.len(),
                batch.alerts.entity.len()
            );
        })
        .await
        .expect("live Amtrak fetch exceeded 180 seconds");
    }
}
