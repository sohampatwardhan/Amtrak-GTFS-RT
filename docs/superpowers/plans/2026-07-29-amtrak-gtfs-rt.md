# Amtrak GTFS-RT Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Rust service that produces spec-valid, live Amtrak GTFS-Realtime (TripUpdates, VehiclePositions, Alerts) plus the static GTFS it binds to, served over HTTP for third-party transit apps.

**Architecture:** A single Cargo binary. It loads Amtrak's official static `GTFS.zip`, then on an interval delegates the hard decrypt→parse→match→encode work to the `catenarytransit/amtrak-gtfs-rt` crate (which returns three ready-made `FeedMessage`s), writes them atomically as `.pb` files, and serves the files with a small `axum` server. A neutral `RtSource` trait wraps the crate so fallback sources can be added later without touching the core. The files on disk *are* the last-good cache: a failed poll cycle simply doesn't overwrite them.

**Tech Stack:** Rust (stable ≥ 1.85), `tokio`, `axum`, `catenarytransit/amtrak-gtfs-rt` (git dep), `gtfs-structures`, `gtfs-realtime`, `prost`, `reqwest`, `async-trait`, `tracing`.

## Global Constraints

- **Rust toolchain:** stable ≥ 1.85 (the `amtrak-gtfs-rt` dependency is edition 2024). Every task assumes this.
- **License:** `AGPL-3.0` (required — we depend on catenary's AGPL-3.0 crate over a network service). `Cargo.toml` `license = "AGPL-3.0"`; repo `LICENSE` file present.
- **Shared-type version pinning:** the types that cross the boundary to catenary's crate must unify.
  - `gtfs-structures = "0.46.1"`, `gtfs-realtime = "0.2.0"`, `prost = "0.14"` MUST each resolve to a **single version** (they carry `Gtfs`, `FeedMessage`, and the protobuf `Message` impl). Verify with `cargo tree -d` — it must show **no** duplicate of these three.
  - `reqwest` MUST match the version catenary's `amtrak-gtfs-rt` uses (currently **0.13**), because the `reqwest::Client` we build is passed into `fetch_amtrak_gtfs_rt(&Gtfs, &reqwest::Client)`. Our `Cargo.toml` pins `reqwest = "0.13"`.
  - **A second `reqwest` (0.12.x) WILL appear in the tree** — `gtfs-structures 0.46.1` depends on it transitively. This duplicate is **expected and acceptable**: that 0.12 copy is internal to `gtfs-structures` and never crosses our boundary as a `Client`. Do not try to eliminate it (it is impossible without forking catenary). Only the three crates above must be single-version.
- **Static feed URL:** `https://content.amtrak.com/content/gtfs/GTFS.zip` (default; overridable via `AMTRAK_STATIC_URL`).
- **Output filenames (exact):** `trip-updates.pb`, `vehicle-positions.pb`, `alerts.pb`, `static.zip`. RT `.pb` content-type `application/protobuf`; zip content-type `application/zip`.
- **All writes atomic:** temp file + rename, never a partial file a consumer could read.

---

### Task 1: Scaffold crate, dependencies, license

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `LICENSE`
- Create: `.gitignore`

**Interfaces:**
- Consumes: nothing.
- Produces: a compiling binary crate with all dependencies resolved to single shared versions.

- [ ] **Step 1: Create `.gitignore`**

```gitignore
/target
/out
```
(This is a binary crate, so `Cargo.lock` **is** committed — it pins the exact resolved versions, which matters given the shared-type version constraint.)

- [ ] **Step 2: Create `Cargo.toml`**

```toml
[package]
name = "amtrak-gtfs-rt-service"
version = "0.1.0"
edition = "2021"
license = "AGPL-3.0"
description = "Serves live GTFS-Realtime feeds for Amtrak"

[dependencies]
amtrak-gtfs-rt = { git = "https://github.com/catenarytransit/amtrak-gtfs-rt" }
gtfs-structures = "0.46.1"
gtfs-realtime = "0.2.0"
prost = "0.14"
reqwest = "0.13"
async-trait = "0.1"
axum = "0.8"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time", "sync", "fs", "signal"] }
tracing = "0.1"
tracing-subscriber = "0.3"
```

- [ ] **Step 3: Create `LICENSE`**

Download the full GNU AGPL-3.0 text from `https://www.gnu.org/licenses/agpl-3.0.txt` and save it verbatim as `LICENSE`.

- [ ] **Step 4: Create `src/main.rs` (temporary stub)**

```rust
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
```

- [ ] **Step 5: Build and verify no duplicate shared crates**

Run: `cargo build`
Expected: compiles (first build downloads the git dep; may take a minute).

Run: `cargo tree -d | grep -E "^(gtfs-structures|gtfs-realtime|prost) v" || echo "NO DUPLICATES"`
Expected: prints `NO DUPLICATES` (these three must be single-version).
A duplicate `reqwest` (0.12.x from `gtfs-structures` alongside 0.13.x from catenary + us) is **expected** — see Global Constraints — so it is deliberately excluded from this check. What matters is that our `reqwest` version matches catenary's: confirm with `cargo tree -i "reqwest@0.13.4"` that the `amtrak-gtfs-rt` git dep sits above the same 0.13.x our crate depends on. If `gtfs-structures`/`gtfs-realtime`/`prost` show a duplicate, align our pin to catenary's version via `cargo metadata` and rebuild until clean.

- [ ] **Step 6: Run the stub test**

Run: `cargo test`
Expected: PASS (1 test).

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs LICENSE .gitignore
git commit -m "chore: scaffold crate with dependencies and AGPL license"
```

---

### Task 2: Configuration

**Files:**
- Create: `src/config.rs`
- Modify: `src/main.rs` (add `mod config;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct Config { pub static_url: String, pub output_dir: std::path::PathBuf, pub poll_interval: std::time::Duration, pub static_refresh_interval: std::time::Duration, pub filter_capital_corridor: bool, pub bind_addr: std::net::SocketAddr }` (derives `Clone, Debug`).
  - `Config::from_env() -> Result<Config, String>`
  - `Config::from_map<F: Fn(&str) -> Option<String>>(get: F) -> Result<Config, String>`

- [ ] **Step 1: Write the failing tests**

Create `src/config.rs`:

```rust
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Config {
    pub static_url: String,
    pub output_dir: std::path::PathBuf,
    pub poll_interval: std::time::Duration,
    pub static_refresh_interval: std::time::Duration,
    pub filter_capital_corridor: bool,
    pub bind_addr: std::net::SocketAddr,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> =
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k: &str| m.get(k).cloned()
    }

    #[test]
    fn defaults_apply_when_env_absent() {
        let c = Config::from_map(map(&[])).unwrap();
        assert_eq!(c.static_url, "https://content.amtrak.com/content/gtfs/GTFS.zip");
        assert_eq!(c.output_dir, std::path::PathBuf::from("./out"));
        assert_eq!(c.poll_interval, std::time::Duration::from_secs(45));
        assert_eq!(c.static_refresh_interval, std::time::Duration::from_secs(86_400));
        assert!(!c.filter_capital_corridor);
        assert_eq!(c.bind_addr, "0.0.0.0:8080".parse().unwrap());
    }

    #[test]
    fn env_overrides_apply() {
        let c = Config::from_map(map(&[
            ("AMTRAK_POLL_SECS", "10"),
            ("AMTRAK_FILTER_CAPITAL_CORRIDOR", "true"),
            ("AMTRAK_BIND_ADDR", "127.0.0.1:9000"),
        ]))
        .unwrap();
        assert_eq!(c.poll_interval, std::time::Duration::from_secs(10));
        assert!(c.filter_capital_corridor);
        assert_eq!(c.bind_addr, "127.0.0.1:9000".parse().unwrap());
    }

    #[test]
    fn invalid_number_errors() {
        let err = Config::from_map(map(&[("AMTRAK_POLL_SECS", "abc")])).unwrap_err();
        assert!(err.contains("AMTRAK_POLL_SECS"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test config`
Expected: FAIL — `from_map` not found.

- [ ] **Step 3: Implement `Config`**

Add above the `#[cfg(test)]` block in `src/config.rs`:

```rust
impl Config {
    pub fn from_env() -> Result<Config, String> {
        Config::from_map(|k| std::env::var(k).ok())
    }

    pub fn from_map<F: Fn(&str) -> Option<String>>(get: F) -> Result<Config, String> {
        let static_url = get("AMTRAK_STATIC_URL")
            .unwrap_or_else(|| "https://content.amtrak.com/content/gtfs/GTFS.zip".to_string());
        let output_dir = get("AMTRAK_OUTPUT_DIR")
            .unwrap_or_else(|| "./out".to_string())
            .into();
        let poll_secs = parse_u64(&get, "AMTRAK_POLL_SECS", 45)?;
        let static_refresh_secs = parse_u64(&get, "AMTRAK_STATIC_REFRESH_SECS", 86_400)?;
        let filter_capital_corridor = get("AMTRAK_FILTER_CAPITAL_CORRIDOR")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let bind_addr = get("AMTRAK_BIND_ADDR")
            .unwrap_or_else(|| "0.0.0.0:8080".to_string())
            .parse()
            .map_err(|e| format!("invalid AMTRAK_BIND_ADDR: {e}"))?;
        Ok(Config {
            static_url,
            output_dir,
            poll_interval: std::time::Duration::from_secs(poll_secs),
            static_refresh_interval: std::time::Duration::from_secs(static_refresh_secs),
            filter_capital_corridor,
            bind_addr,
        })
    }
}

fn parse_u64<F: Fn(&str) -> Option<String>>(get: &F, key: &str, default: u64) -> Result<u64, String> {
    match get(key) {
        Some(v) => v.parse().map_err(|e| format!("invalid {key}: {e}")),
        None => Ok(default),
    }
}
```

Add `mod config;` to the top of `src/main.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test config`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/config.rs src/main.rs
git commit -m "feat: env-driven configuration"
```

---

### Task 3: Atomic file writer

**Files:**
- Create: `src/writer.rs`
- Modify: `src/main.rs` (add `mod writer;`)

**Interfaces:**
- Consumes: nothing.
- Produces: `pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()>`

- [ ] **Step 1: Write the failing tests**

Create `src/writer.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("amtrak-writer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.pb");
        write_atomic(&path, b"hello").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    }

    #[test]
    fn overwrites_existing() {
        let dir = std::env::temp_dir().join(format!("amtrak-writer-ow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.pb");
        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        // temp file must not linger
        assert!(!path.with_extension("tmp").exists());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test writer`
Expected: FAIL — `write_atomic` not found.

- [ ] **Step 3: Implement `write_atomic`**

Add above the `#[cfg(test)]` block in `src/writer.rs`:

```rust
use std::path::Path;

/// Write `bytes` to `path` atomically: write to a sibling `.tmp` file, then rename.
/// A consumer reading `path` therefore never sees a partial write.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
```

Add `mod writer;` to `src/main.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test writer`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/writer.rs src/main.rs
git commit -m "feat: atomic file writer"
```

---

### Task 4: RtSource seam and RtBatch

**Files:**
- Create: `src/sources/mod.rs`
- Modify: `src/main.rs` (add `mod sources;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct RtBatch { pub trip_updates: gtfs_realtime::FeedMessage, pub vehicle_positions: gtfs_realtime::FeedMessage, pub alerts: gtfs_realtime::FeedMessage }` (derives `Clone`).
  - `impl RtBatch { pub fn empty() -> RtBatch; pub fn is_empty(&self) -> bool }`
  - `pub type SourceError = Box<dyn std::error::Error + Send + Sync>;`
  - `#[async_trait] pub trait RtSource: Send + Sync { fn name(&self) -> &'static str; async fn fetch(&self, gtfs: &gtfs_structures::Gtfs) -> Result<RtBatch, SourceError>; }`
  - Test-only: `pub mod mock` with `MockSource { name, behavior }`, `enum Behavior { Ok(RtBatch), Empty, Fail }`, `fn batch_with(n: usize) -> RtBatch`.

- [ ] **Step 1: Write the failing tests**

Create `src/sources/mod.rs`:

```rust
use async_trait::async_trait;
use gtfs_realtime::FeedMessage;
use gtfs_structures::Gtfs;

pub type SourceError = Box<dyn std::error::Error + Send + Sync>;

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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test sources`
Expected: FAIL — `RtBatch`, `mock`, etc. not found.

- [ ] **Step 3: Implement the seam**

Add above the `#[cfg(test)]` block in `src/sources/mod.rs`:

```rust
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
```

Add `mod sources;` to `src/main.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test sources`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/sources/mod.rs src/main.rs
git commit -m "feat: RtSource trait and RtBatch normalization model"
```

---

### Task 5: Amtrak source (wraps catenary crate)

**Files:**
- Create: `src/sources/amtrak.rs`
- Modify: `src/sources/mod.rs` (add `pub mod amtrak;`)

**Interfaces:**
- Consumes: `RtSource`, `RtBatch`, `SourceError` from Task 4.
- Produces:
  - `pub struct AmtrakSource { /* holds a reqwest::Client */ }`
  - `impl AmtrakSource { pub fn new() -> AmtrakSource }`
  - `impl RtSource for AmtrakSource` with `name() == "amtrak"`, `fetch` delegating to `amtrak_gtfs_rt::fetch_amtrak_gtfs_rt`.

- [ ] **Step 1: Write the failing test**

Create `src/sources/amtrak.rs`:

```rust
use crate::sources::{RtBatch, RtSource, SourceError};
use async_trait::async_trait;
use gtfs_structures::Gtfs;

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
        let gtfs =
            Gtfs::from_url_async("https://content.amtrak.com/content/gtfs/GTFS.zip")
                .await
                .unwrap();
        let src = AmtrakSource::new();
        let batch = src.fetch(&gtfs).await.unwrap();
        // At virtually any hour some Amtrak trains are running.
        assert!(!batch.is_empty(), "expected at least one live train");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test sources::amtrak::tests::source_is_named_amtrak`
Expected: FAIL — `AmtrakSource` not found.

- [ ] **Step 3: Implement `AmtrakSource`**

Add above the `#[cfg(test)]` block in `src/sources/amtrak.rs`:

```rust
/// Primary realtime source: delegates to catenary's `amtrak-gtfs-rt` crate,
/// which fetches and decrypts Amtrak's `getTrainsData`, matches trains to GTFS
/// trips (handling multi-day date offsets), and returns ready-made FeedMessages.
pub struct AmtrakSource {
    client: reqwest::Client,
}

impl AmtrakSource {
    pub fn new() -> AmtrakSource {
        AmtrakSource { client: reqwest::Client::new() }
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
```

Add `pub mod amtrak;` to `src/sources/mod.rs` (below the trait definition, before the test module).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test sources::amtrak::tests::source_is_named_amtrak`
Expected: PASS.

Then the live integration check:
Run: `cargo test sources::amtrak -- --ignored --nocapture`
Expected: PASS — confirms the catenary crate, decryption, and static binding all work end-to-end against the real endpoints. (If it fails due to a transient Amtrak outage, retry; if it fails to compile due to a `reqwest::Client` type mismatch, revisit Task 1 Step 5 version alignment.)

- [ ] **Step 5: Commit**

```bash
git add src/sources/amtrak.rs src/sources/mod.rs
git commit -m "feat: Amtrak realtime source via catenary crate"
```

---

### Task 6: Static GTFS ingest and shared store

**Files:**
- Create: `src/static_gtfs.rs`
- Modify: `src/main.rs` (add `mod static_gtfs;`)

**Interfaces:**
- Consumes: `crate::writer::write_atomic` (Task 3), `crate::config::Config` (Task 2).
- Produces:
  - `pub struct StaticFeed { pub gtfs: std::sync::Arc<gtfs_structures::Gtfs>, pub feed_version: String }` (derives `Clone`).
  - `pub struct SharedStore<T> { .. }` with manual `Clone`, and `impl<T: Clone> SharedStore<T> { pub fn new(v: T) -> Self; pub async fn get(&self) -> T; pub async fn set(&self, v: T) }`.
  - `pub type StaticFeedStore = SharedStore<StaticFeed>;`
  - `pub async fn load_static_feed(url: &str) -> Result<StaticFeed, Box<dyn std::error::Error + Send + Sync>>`
  - `pub async fn save_static_zip(url: &str, dest: &std::path::Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>>`
  - `pub async fn run_static_refresh(store: StaticFeedStore, config: crate::config::Config)`

- [ ] **Step 1: Write the failing tests**

Create `src/static_gtfs.rs`:

```rust
use gtfs_structures::Gtfs;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shared_store_round_trips() {
        let store: SharedStore<String> = SharedStore::new("a".to_string());
        assert_eq!(store.get().await, "a");
        store.set("b".to_string()).await;
        assert_eq!(store.get().await, "b");
    }

    #[tokio::test]
    async fn shared_store_clone_shares_state() {
        let store: SharedStore<String> = SharedStore::new("a".to_string());
        let clone = store.clone();
        clone.set("b".to_string()).await;
        // The clone shares the same inner Arc<RwLock>, so the original sees the update.
        assert_eq!(store.get().await, "b");
    }

    // Live test: downloads Amtrak's real GTFS.zip (~19 MB).
    //   cargo test static_gtfs -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn live_load_static_feed() {
        let feed =
            load_static_feed("https://content.amtrak.com/content/gtfs/GTFS.zip").await.unwrap();
        assert!(!feed.gtfs.trips.is_empty(), "static feed should have trips");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test static_gtfs`
Expected: FAIL — `SharedStore` not found.

- [ ] **Step 3: Implement the store and loaders**

Add above the `#[cfg(test)]` block in `src/static_gtfs.rs`:

```rust
#[derive(Clone)]
pub struct StaticFeed {
    pub gtfs: Arc<Gtfs>,
    pub feed_version: String,
}

/// A cheaply-clonable handle to a shared, swappable value. Clones share one
/// inner lock, so a background refresh task can replace the value while readers
/// hold their own handle.
pub struct SharedStore<T> {
    inner: Arc<RwLock<T>>,
}

impl<T> Clone for SharedStore<T> {
    fn clone(&self) -> Self {
        SharedStore { inner: self.inner.clone() }
    }
}

impl<T: Clone> SharedStore<T> {
    pub fn new(v: T) -> Self {
        SharedStore { inner: Arc::new(RwLock::new(v)) }
    }
    pub async fn get(&self) -> T {
        self.inner.read().await.clone()
    }
    pub async fn set(&self, v: T) {
        *self.inner.write().await = v;
    }
}

pub type StaticFeedStore = SharedStore<StaticFeed>;

/// Download and parse Amtrak's static GTFS into an in-memory `Gtfs`.
pub async fn load_static_feed(
    url: &str,
) -> Result<StaticFeed, Box<dyn std::error::Error + Send + Sync>> {
    let gtfs = Gtfs::from_url_async(url).await?;
    let feed_version = gtfs
        .feed_info
        .first()
        .and_then(|fi| fi.version.clone())
        .unwrap_or_else(|| "unknown".to_string());
    Ok(StaticFeed { gtfs: Arc::new(gtfs), feed_version })
}

/// Download the raw GTFS.zip bytes and write them to `dest` so the static feed
/// can be served alongside the realtime feeds.
pub async fn save_static_zip(
    url: &str,
    dest: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let bytes = reqwest::get(url).await?.error_for_status()?.bytes().await?;
    crate::writer::write_atomic(dest, &bytes)?;
    Ok(())
}

/// Periodically refresh the static feed. Keeps the last-good feed on failure.
pub async fn run_static_refresh(store: StaticFeedStore, config: crate::config::Config) {
    let mut ticker = tokio::time::interval(config.static_refresh_interval);
    ticker.tick().await; // the first tick fires immediately; skip it (already loaded at startup)
    loop {
        ticker.tick().await;
        match load_static_feed(&config.static_url).await {
            Ok(feed) => {
                let version = feed.feed_version.clone();
                store.set(feed).await;
                if let Err(e) =
                    save_static_zip(&config.static_url, &config.output_dir.join("static.zip")).await
                {
                    tracing::error!(error = %e, "failed to save static.zip");
                }
                tracing::info!(feed_version = %version, "refreshed static feed");
            }
            Err(e) => tracing::error!(error = %e, "static refresh failed; keeping last-good"),
        }
    }
}
```

Add `mod static_gtfs;` to `src/main.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test static_gtfs`
Expected: PASS (2 non-ignored tests).

Optional live check:
Run: `cargo test static_gtfs -- --ignored --nocapture`
Expected: PASS — downloads and parses the real feed.

- [ ] **Step 5: Commit**

```bash
git add src/static_gtfs.rs src/main.rs
git commit -m "feat: static GTFS ingest with swappable shared store"
```

---

### Task 7: Orchestrator (source chain, encode, write)

**Files:**
- Create: `src/orchestrator.rs`
- Modify: `src/main.rs` (add `mod orchestrator;`)

**Interfaces:**
- Consumes: `RtSource`, `RtBatch` (Task 4); `write_atomic` (Task 3); `Config` (Task 2); `StaticFeedStore` (Task 6); `amtrak_gtfs_rt::filter_capital_corridor`.
- Produces:
  - `pub async fn select_batch(sources: &[Box<dyn RtSource>], gtfs: &gtfs_structures::Gtfs) -> Option<(&'static str, RtBatch)>`
  - `pub fn write_feeds(dir: &std::path::Path, batch: RtBatch, filter_capital_corridor: bool, feed_version: &str) -> std::io::Result<()>`
  - `pub async fn run_poller(sources: std::sync::Arc<Vec<Box<dyn RtSource>>>, store: StaticFeedStore, config: crate::config::Config)`

- [ ] **Step 1: Write the failing tests**

Create `src/orchestrator.rs`:

```rust
use crate::sources::{RtBatch, RtSource};
use crate::static_gtfs::StaticFeedStore;
use gtfs_structures::Gtfs;
use prost::Message;
use std::path::Path;

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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test orchestrator`
Expected: FAIL — `select_batch` / `write_feeds` not found.

- [ ] **Step 3: Implement the orchestrator**

Add above the `#[cfg(test)]` block in `src/orchestrator.rs`:

```rust
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
```

Add `mod orchestrator;` to `src/main.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test orchestrator`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add src/orchestrator.rs src/main.rs
git commit -m "feat: orchestrator with source chain and atomic feed writing"
```

---

### Task 8: HTTP serving

**Files:**
- Create: `src/serve.rs`
- Modify: `src/main.rs` (add `mod serve;`)

**Interfaces:**
- Consumes: `crate::config::Config` (Task 2).
- Produces:
  - `pub fn router(dir: std::path::PathBuf) -> axum::Router`
  - `pub async fn run_server(config: crate::config::Config) -> std::io::Result<()>`

- [ ] **Step 1: Write the failing test**

Create `src/serve.rs`:

```rust
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use std::path::PathBuf;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serves_pb_and_zip_with_content_types_and_404() {
        let dir = std::env::temp_dir().join(format!("amtrak-serve-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("trip-updates.pb"), b"\x08\x01").unwrap();
        std::fs::write(dir.join("static.zip"), b"PK\x03\x04").unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router(dir)).await.unwrap() });

        let client = reqwest::Client::new();

        let health = client.get(format!("http://{addr}/health")).send().await.unwrap();
        assert_eq!(health.status(), 200);

        let tu = client.get(format!("http://{addr}/trip-updates.pb")).send().await.unwrap();
        assert_eq!(tu.status(), 200);
        assert_eq!(tu.headers()[header::CONTENT_TYPE], "application/protobuf");
        assert_eq!(tu.bytes().await.unwrap().as_ref(), b"\x08\x01");

        let zip = client.get(format!("http://{addr}/static.zip")).send().await.unwrap();
        assert_eq!(zip.headers()[header::CONTENT_TYPE], "application/zip");

        let missing = client.get(format!("http://{addr}/alerts.pb")).send().await.unwrap();
        assert_eq!(missing.status(), 404);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test serve`
Expected: FAIL — `router` not found.

- [ ] **Step 3: Implement the server**

Add above the `#[cfg(test)]` block in `src/serve.rs`:

```rust
#[derive(Clone)]
struct ServeState {
    dir: PathBuf,
}

pub fn router(dir: PathBuf) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/trip-updates.pb", get(trip_updates))
        .route("/vehicle-positions.pb", get(vehicle_positions))
        .route("/alerts.pb", get(alerts))
        .route("/static.zip", get(static_zip))
        .with_state(ServeState { dir })
}

async fn health() -> &'static str {
    "ok"
}

async fn serve_file(dir: &std::path::Path, name: &str, content_type: &'static str) -> Response {
    match tokio::fs::read(dir.join(name)).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, content_type)], bytes).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn trip_updates(State(s): State<ServeState>) -> Response {
    serve_file(&s.dir, "trip-updates.pb", "application/protobuf").await
}
async fn vehicle_positions(State(s): State<ServeState>) -> Response {
    serve_file(&s.dir, "vehicle-positions.pb", "application/protobuf").await
}
async fn alerts(State(s): State<ServeState>) -> Response {
    serve_file(&s.dir, "alerts.pb", "application/protobuf").await
}
async fn static_zip(State(s): State<ServeState>) -> Response {
    serve_file(&s.dir, "static.zip", "application/zip").await
}

pub async fn run_server(config: crate::config::Config) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(addr = %config.bind_addr, "serving feeds");
    axum::serve(listener, router(config.output_dir)).await
}
```

Add `mod serve;` to `src/main.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test serve`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/serve.rs src/main.rs
git commit -m "feat: axum server for feed files"
```

---

### Task 9: Wire main and end-to-end verification

**Files:**
- Modify: `src/main.rs` (replace the stub `main`)
- Modify: `README.md`

**Interfaces:**
- Consumes: everything produced above.
- Produces: a runnable binary that loads the static feed, spawns the refresh + poll + serve tasks, and serves live feeds.

- [ ] **Step 1: Replace `main`**

Replace the stub `fn main()` and its test module in `src/main.rs` with (keep the `mod` declarations at the top):

```rust
use crate::config::Config;
use crate::sources::amtrak::AmtrakSource;
use crate::sources::RtSource;
use crate::static_gtfs::{load_static_feed, save_static_zip, StaticFeedStore};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env().map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
        e.into()
    })?;
    std::fs::create_dir_all(&config.output_dir)?;

    let feed = load_static_feed(&config.static_url).await?;
    tracing::info!(feed_version = %feed.feed_version, "loaded static feed");
    save_static_zip(&config.static_url, &config.output_dir.join("static.zip")).await?;
    let store = StaticFeedStore::new(feed);

    let sources: Arc<Vec<Box<dyn RtSource>>> = Arc::new(vec![Box::new(AmtrakSource::new())]);

    let poller = tokio::spawn(orchestrator::run_poller(
        sources.clone(),
        store.clone(),
        config.clone(),
    ));
    let refresher = tokio::spawn(static_gtfs::run_static_refresh(store.clone(), config.clone()));
    let server = tokio::spawn(serve::run_server(config.clone()));

    // If any long-lived task exits, shut the process down so a supervisor restarts it.
    tokio::select! {
        _ = poller => tracing::error!("poller task exited"),
        _ = refresher => tracing::error!("refresh task exited"),
        r = server => tracing::error!(result = ?r, "server task exited"),
    }
    Ok(())
}
```

Ensure the top of `src/main.rs` still declares all modules:

```rust
mod config;
mod orchestrator;
mod serve;
mod sources;
mod static_gtfs;
mod writer;
```

- [ ] **Step 2: Build**

Run: `cargo build`
Expected: compiles with no errors.

- [ ] **Step 3: Full test suite**

Run: `cargo test`
Expected: all non-ignored tests PASS.

- [ ] **Step 4: End-to-end smoke run (manual)**

Run:
```bash
AMTRAK_POLL_SECS=30 AMTRAK_OUTPUT_DIR=./out AMTRAK_BIND_ADDR=127.0.0.1:8080 cargo run
```
Expected logs: `loaded static feed`, then within ~30 s `wrote feeds source=amtrak trip_updates=<n> vehicles=<n> ...`.

In another shell:
```bash
curl -s -o /dev/null -w "%{http_code} %{content_type}\n" http://127.0.0.1:8080/vehicle-positions.pb
curl -s -o /dev/null -w "%{http_code} %{content_type}\n" http://127.0.0.1:8080/static.zip
```
Expected: `200 application/protobuf` and `200 application/zip`.

- [ ] **Step 5: Validate the feeds (spec compliance gate)**

Validate the produced protobuf decodes and the static feed is spec-valid:
```bash
# static: use any GTFS validator, e.g. MobilityData's:
#   https://github.com/MobilityData/gtfs-validator  (java -jar gtfs-validator.jar -i out/static.zip -o report)
# realtime: MobilityData's gtfs-realtime-validator, pointed at the served URLs:
#   http://127.0.0.1:8080/trip-updates.pb, /vehicle-positions.pb, /alerts.pb
```
Expected: RT validator reports the feeds parse and reference `trip_id`/`stop_id`/`route_id` values that exist in the static feed (no unresolved-ID errors). Record the validator summary in the commit message.

- [ ] **Step 6: Update `README.md`**

Replace `README.md` with a short usage doc: what the service does, the served endpoints (`/trip-updates.pb`, `/vehicle-positions.pb`, `/alerts.pb`, `/static.zip`, `/health`), the env vars (`AMTRAK_STATIC_URL`, `AMTRAK_OUTPUT_DIR`, `AMTRAK_POLL_SECS`, `AMTRAK_STATIC_REFRESH_SECS`, `AMTRAK_FILTER_CAPITAL_CORRIDOR`, `AMTRAK_BIND_ADDR`), how to run (`cargo run`), and an **AGPL-3.0** notice with attribution to `catenarytransit/amtrak-gtfs-rt`.

- [ ] **Step 7: Commit**

```bash
git add src/main.rs README.md
git commit -m "feat: wire service entrypoint and document usage"
```

---

## Deferred (not in this plan — see spec §9)

- `TransitDocsSource` / `RailRatSource` positional fallback implementations (the `RtSource` seam is ready for them — add a file per source under `src/sources/` and push into the `sources` vec in `main`).
- ETag-conditional static refresh (currently a full daily re-download).
- Hosting/CDN, monitoring dashboards, historical archival.
- True match-rate metric (needs the raw pre-transform observation count, which the catenary crate does not expose; v1 logs entity counts per cycle instead).
