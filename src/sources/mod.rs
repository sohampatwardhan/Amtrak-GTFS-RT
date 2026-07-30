use async_trait::async_trait;
use gtfs_realtime::FeedMessage;
use gtfs_structures::Gtfs;

pub type SourceError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone, Debug)]
pub struct RtBatch {
    pub trip_updates: FeedMessage,
    pub vehicle_positions: FeedMessage,
    pub alerts: FeedMessage,
}

impl RtBatch {
    pub fn empty() -> RtBatch {
        RtBatch {
            trip_updates: FeedMessage::default(),
            vehicle_positions: FeedMessage::default(),
            alerts: FeedMessage::default(),
        }
    }

    /// A batch is empty when no source produced any entities. The orchestrator
    /// treats an empty batch as "no fresh data" and advances to the next source.
    pub fn is_empty(&self) -> bool {
        self.trip_updates.entity.is_empty()
            && self.vehicle_positions.entity.is_empty()
            && self.alerts.entity.is_empty()
    }
}

/// A realtime data source. Implementations normalize their provider's data into
/// an `RtBatch` so the orchestrator never depends on which provider produced it.
#[async_trait]
pub trait RtSource: Send + Sync {
    fn name(&self) -> &'static str;
    async fn fetch(&self, gtfs: &Gtfs) -> Result<RtBatch, SourceError>;
}

#[cfg(test)]
pub mod mock {
    use super::*;
    use gtfs_realtime::{FeedEntity, FeedMessage};

    pub enum Behavior {
        Ok(RtBatch),
        Empty,
        Fail,
    }

    pub struct MockSource {
        pub name: &'static str,
        pub behavior: Behavior,
    }

    #[async_trait]
    impl RtSource for MockSource {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn fetch(&self, _gtfs: &Gtfs) -> Result<RtBatch, SourceError> {
            match &self.behavior {
                Behavior::Ok(b) => Ok(b.clone()),
                Behavior::Empty => Ok(RtBatch::empty()),
                Behavior::Fail => Err("mock failure".into()),
            }
        }
    }

    /// Build an RtBatch whose trip_updates and vehicle_positions each carry `n`
    /// entities (alerts left empty), for exercising non-empty paths.
    pub fn batch_with(n: usize) -> RtBatch {
        let mut m = FeedMessage::default();
        for i in 0..n {
            m.entity.push(FeedEntity { id: i.to_string(), ..Default::default() });
        }
        RtBatch {
            trip_updates: m.clone(),
            vehicle_positions: m,
            alerts: FeedMessage::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::{batch_with, Behavior, MockSource};
    use super::*;

    #[test]
    fn empty_batch_is_empty() {
        assert!(RtBatch::empty().is_empty());
    }

    #[test]
    fn batch_with_entities_is_not_empty() {
        assert!(!batch_with(2).is_empty());
    }

    #[tokio::test]
    async fn mock_source_reports_name_and_batch() {
        let src = MockSource { name: "mock", behavior: Behavior::Ok(batch_with(1)) };
        assert_eq!(src.name(), "mock");
        let batch = src.fetch(&Gtfs::default()).await.unwrap();
        assert_eq!(batch.trip_updates.entity.len(), 1);
    }

    #[tokio::test]
    async fn mock_source_can_fail() {
        let src = MockSource { name: "bad", behavior: Behavior::Fail };
        assert!(src.fetch(&Gtfs::default()).await.is_err());
    }
}
