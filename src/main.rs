//! Process entry for `amtrak-gtfs-rt-service`.
//!
//! `AMTRAK_ROLE` selects VehiclePositions, TripUpdates, Alerts, or the combined
//! process. The binary is the same in every container; the image and the role
//! decide which schedulers run.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    amtrak_gtfs_rt_service::run_service().await
}
