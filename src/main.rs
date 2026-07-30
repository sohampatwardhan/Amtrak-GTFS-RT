mod config;
mod orchestrator;
mod sources;
mod static_gtfs;
mod writer;

fn main() {
    println!("amtrak-gtfs-rt-service");
}

#[cfg(test)]
mod tests {
    #[test]
    fn scaffold_builds() {
        assert_eq!(2 + 2, 4);
    }
}
