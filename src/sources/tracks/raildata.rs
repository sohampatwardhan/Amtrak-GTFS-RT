//! Registered NJ Transit RailData access for Amtrak tracks at NJT stations.
//!
//! NJ Transit's sanctioned developer access is a RailData account: `getToken` exchanges the
//! account's username and password for a token valid for 24 hours, and `getTrainSchedule19Rec`
//! returns a station's next 19 departures, including Amtrak trains (`TRAIN_ID` prefixed `A`) and
//! their `TRACK`. NJT allows only 10 `getToken` calls per day, so the token is cached on disk and
//! reused across restarts, and token requests are counted in a rolling 24-hour window that is
//! stricter than NJT's midnight reset. Nothing here reproduces the DepartureVision web client.
//!
//! Credentials and tokens never appear in logs or errors: [`BoardError`] carries only a kind.

use super::store::Assignment;
use super::{is_track_label, normalize_train_number};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A cached token is reused while younger than this, one hour short of NJT's 24-hour validity so a
/// token never expires mid-refresh.
pub const TOKEN_REUSE: Duration = Duration::from_secs(23 * 3600);
/// Window over which token requests are counted.
pub const TOKEN_BUDGET_WINDOW: Duration = Duration::from_secs(24 * 3600);
/// Maximum token requests in [`TOKEN_BUDGET_WINDOW`]; NJT's documented daily limit.
pub const TOKEN_BUDGET: usize = 10;
/// Pause after NJT rejects the credentials, so a wrong password cannot spend the day's budget.
pub const CREDENTIAL_SUSPENSION: Duration = Duration::from_secs(3600);

/// Why a RailData request produced no usable schedule. Deliberately carries no response text.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BoardError {
    /// No username/password configured.
    Unconfigured,
    /// Requests are paused after rejected credentials.
    Suspended,
    /// The rolling token budget is spent.
    BudgetExhausted,
    /// NJT answered `Authenticated: "False"`.
    CredentialsRejected,
    /// Transport failure, timeout, or non-success status.
    Http,
    /// The response was not a usable schedule.
    Unusable,
}

/// On-disk token state: the current token, when it was issued, and recent request times.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TokenState {
    token: Option<String>,
    issued_at: Option<u64>,
    requests: Vec<u64>,
}

/// Token cache persisted as JSON (`token`, `issued_at`, `requests`) with owner-only permissions.
///
/// A missing or unreadable file starts empty. Writes go to a temporary file that is renamed into
/// place, so a crash never leaves a half-written token.
pub struct TokenCache {
    path: PathBuf,
    state: TokenState,
}

impl TokenCache {
    /// Loads the cache at `path`, or starts empty when it is missing or malformed.
    pub fn load(path: PathBuf) -> TokenCache {
        let state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map(|value| TokenState {
                token: value
                    .get("token")
                    .and_then(Value::as_str)
                    .filter(|token| !token.is_empty())
                    .map(str::to_string),
                issued_at: value.get("issued_at").and_then(Value::as_u64),
                requests: value
                    .get("requests")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().filter_map(Value::as_u64).collect())
                    .unwrap_or_default(),
            })
            .unwrap_or_default();
        TokenCache { path, state }
    }

    fn usable_token(&self, now: SystemTime) -> Option<String> {
        let issued = self.state.issued_at?;
        let age = unix(now).checked_sub(issued)?;
        (age < TOKEN_REUSE.as_secs())
            .then(|| self.state.token.clone())
            .flatten()
    }

    fn prune(&mut self, now: SystemTime) {
        let cutoff = unix(now).saturating_sub(TOKEN_BUDGET_WINDOW.as_secs());
        self.state.requests.retain(|at| *at > cutoff);
    }

    fn can_request(&mut self, now: SystemTime) -> bool {
        self.prune(now);
        self.state.requests.len() < TOKEN_BUDGET
    }

    fn record_request(&mut self, now: SystemTime) {
        self.state.requests.push(unix(now));
    }

    fn store_token(&mut self, token: String, now: SystemTime) {
        self.state.token = Some(token);
        self.state.issued_at = Some(unix(now));
    }

    fn clear_token(&mut self) {
        self.state.token = None;
        self.state.issued_at = None;
    }

    fn save(&self) {
        if write_private(&self.path, &self.render()).is_err() {
            tracing::warn!("raildata token cache could not be written");
        }
    }

    fn render(&self) -> String {
        serde_json::json!({
            "token": self.state.token,
            "issued_at": self.state.issued_at,
            "requests": self.state.requests,
        })
        .to_string()
    }
}

fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)
}

fn unix(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// RailData client owning the token cache and budget.
pub struct RailDataClient {
    http: reqwest::Client,
    base: String,
    credentials: Option<(String, String)>,
    cache: TokenCache,
    suspended_until: Option<SystemTime>,
}

impl RailDataClient {
    /// Creates a client for `base` (the `TrainData` URL) whose token cache lives at `token_path`.
    ///
    /// `http` should carry the per-request timeout; `credentials` is `None` when unconfigured.
    pub fn new(
        http: reqwest::Client,
        base: &str,
        credentials: Option<(String, String)>,
        token_path: PathBuf,
    ) -> RailDataClient {
        RailDataClient {
            http,
            base: base.trim_end_matches('/').to_string(),
            credentials,
            cache: TokenCache::load(token_path),
            suspended_until: None,
        }
    }

    /// Whether registered credentials are configured.
    pub fn is_configured(&self) -> bool {
        self.credentials.is_some()
    }

    /// Returns one station's `getTrainSchedule19Rec` JSON.
    ///
    /// Reuses the cached token; requests a token only when none is usable and the budget allows.
    /// When NJT rejects the token, requests exactly one replacement and retries once.
    ///
    /// # Errors
    ///
    /// Returns the [`BoardError`] kind; no response text or secret is included.
    pub async fn schedule(&mut self, station: &str, now: SystemTime) -> Result<String, BoardError> {
        if self.credentials.is_none() {
            return Err(BoardError::Unconfigured);
        }
        if self.suspended_until.is_some_and(|until| now < until) {
            return Err(BoardError::Suspended);
        }
        let token = match self.cache.usable_token(now) {
            Some(token) => token,
            None => self.request_token(now).await?,
        };
        let body = self.fetch_schedule(&token, station).await?;
        if !token_rejected(&body) {
            return Ok(body);
        }
        self.cache.clear_token();
        self.cache.save();
        let token = self.request_token(now).await?;
        let body = self.fetch_schedule(&token, station).await?;
        if token_rejected(&body) {
            return Err(BoardError::Unusable);
        }
        Ok(body)
    }

    async fn fetch_schedule(&self, token: &str, station: &str) -> Result<String, BoardError> {
        post_multipart(
            &self.http,
            &format!("{}/getTrainSchedule19Rec", self.base),
            &[("token", token), ("station", station), ("line", "")],
        )
        .await
    }

    async fn request_token(&mut self, now: SystemTime) -> Result<String, BoardError> {
        let Some((username, password)) = self.credentials.clone() else {
            return Err(BoardError::Unconfigured);
        };
        if !self.cache.can_request(now) {
            return Err(BoardError::BudgetExhausted);
        }
        // Counted before the call so a crash mid-request still spends budget, matching NJT's view.
        self.cache.record_request(now);
        self.cache.save();
        let body = post_multipart(
            &self.http,
            &format!("{}/getToken", self.base),
            &[("username", &username), ("password", &password)],
        )
        .await?;
        let value: Value = serde_json::from_str(&body).map_err(|_| BoardError::Unusable)?;
        if value.get("errorMessage").is_some() {
            return Err(BoardError::BudgetExhausted);
        }
        let authenticated = value
            .get("Authenticated")
            .and_then(Value::as_str)
            .is_some_and(|flag| flag.eq_ignore_ascii_case("true"));
        let token = value
            .get("UserToken")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty());
        match (authenticated, token) {
            (true, Some(token)) => {
                self.cache.store_token(token.to_string(), now);
                self.cache.save();
                Ok(token.to_string())
            }
            _ => {
                self.suspended_until = Some(now + CREDENTIAL_SUSPENSION);
                Err(BoardError::CredentialsRejected)
            }
        }
    }
}

/// Whether a schedule response means the token was not accepted: `null`, empty, or an
/// `errorMessage` naming the token.
fn token_rejected(body: &str) -> bool {
    let trimmed = body.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return true;
    }
    serde_json::from_str::<Value>(trimmed)
        .ok()
        .and_then(|value| {
            value
                .get("errorMessage")
                .and_then(Value::as_str)
                .map(|message| message.to_ascii_lowercase().contains("token"))
        })
        .unwrap_or(false)
}

async fn post_multipart(
    http: &reqwest::Client,
    url: &str,
    fields: &[(&str, &str)],
) -> Result<String, BoardError> {
    let boundary = "amtraktrackboundary";
    let mut body = String::new();
    for (name, value) in fields {
        body.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    body.push_str(&format!("--{boundary}--\r\n"));
    let response = http
        .post(url)
        .header(
            reqwest::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .map_err(|_| BoardError::Http)?;
    if !response.status().is_success() {
        return Err(BoardError::Http);
    }
    response.text().await.map_err(|_| BoardError::Http)
}

/// Amtrak rows from one `getTrainSchedule19Rec` document, keyed by the mapped Amtrak stop.
///
/// Only `TRAIN_ID`s of the form `A<digits>` are Amtrak; rows whose `TRACK` is empty or not a
/// platform token (for example `TBD`) are discarded.
///
/// # Errors
///
/// Returns [`BoardError::Unusable`] when the station is unmapped or the body is not a schedule.
pub fn assignments_for_station(
    body: &str,
    njt_station: &str,
    station_map: &[(String, String)],
) -> Result<Vec<Assignment>, BoardError> {
    let stop_id = station_map
        .iter()
        .find_map(|(code, amtrak)| {
            code.eq_ignore_ascii_case(njt_station)
                .then(|| amtrak.clone())
        })
        .ok_or(BoardError::Unusable)?;
    let value: Value = serde_json::from_str(body.trim()).map_err(|_| BoardError::Unusable)?;
    if value.get("errorMessage").is_some() {
        return Err(BoardError::Unusable);
    }
    let items = items_of(&value).ok_or(BoardError::Unusable)?;
    let mut rows = Vec::new();
    for object in items.iter().filter_map(Value::as_object) {
        let Some(train_number) = object_field(object, "TRAIN_ID").and_then(amtrak_number_from_njt)
        else {
            continue;
        };
        let Some(track) = object_field(object, "TRACK").map(|track| track.trim().to_uppercase())
        else {
            continue;
        };
        if !is_track_label(&track) {
            continue;
        }
        rows.push(Assignment {
            train_number,
            stop_id: stop_id.clone(),
            track,
        });
    }
    Ok(rows)
}

/// Amtrak train number from an NJT `TRAIN_ID` (`A67`, `A067` → `67`); `None` for NJT trains.
pub fn amtrak_number_from_njt(train_id: &str) -> Option<String> {
    let trimmed = train_id.trim();
    let rest = trimmed
        .strip_prefix('A')
        .or_else(|| trimmed.strip_prefix('a'))?;
    if rest.is_empty() || !rest.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    normalize_train_number(rest)
}

fn items_of(value: &Value) -> Option<&Vec<Value>> {
    match value {
        Value::Array(items) => Some(items),
        Value::Object(object) => object
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("items"))
            .and_then(|(_, item)| item.as_array()),
        _ => None,
    }
}

fn object_field<'a>(object: &'a serde_json::Map<String, Value>, name: &str) -> Option<&'a str> {
    object
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(_, value)| value.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::routing::post;
    use axum::Router;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn map() -> Vec<(String, String)> {
        vec![("NP".into(), "NWK".into())]
    }

    #[test]
    fn njt_fixture_keeps_amtrak_tracks_only() {
        let body = include_str!("../../../fixtures/tracks/njt_newark.json");
        let rows = assignments_for_station(body, "NP", &map()).unwrap();
        let tracks: Vec<_> = rows
            .iter()
            .map(|row| {
                (
                    row.train_number.as_str(),
                    row.track.as_str(),
                    row.stop_id.as_str(),
                )
            })
            .collect();
        assert_eq!(
            tracks,
            vec![("67", "4", "NWK"), ("67", "2", "NWK"), ("169", "3", "NWK")]
        );
        assert_eq!(
            assignments_for_station(r#"{"errorMessage":"Invalid token."}"#, "NP", &map()),
            Err(BoardError::Unusable)
        );
        assert_eq!(
            assignments_for_station("{}", "ZZ", &map()),
            Err(BoardError::Unusable)
        );
    }

    #[test]
    fn njt_train_ids_need_the_amtrak_prefix() {
        assert_eq!(amtrak_number_from_njt("7885"), None);
        assert_eq!(amtrak_number_from_njt("A2228").as_deref(), Some("2228"));
        assert_eq!(amtrak_number_from_njt("a067").as_deref(), Some("67"));
    }

    #[test]
    fn token_rejection_is_recognized() {
        assert!(token_rejected("null"));
        assert!(token_rejected("  "));
        assert!(token_rejected(r#"{"errorMessage":"Invalid token."}"#));
        assert!(!token_rejected(r#"{"STATION_2CHAR":"NP","ITEMS":[]}"#));
    }

    /// Scripted RailData stand-in: counts calls and records the tokens schedules were sent with.
    #[derive(Default)]
    struct Fake {
        token_calls: AtomicUsize,
        schedule_calls: AtomicUsize,
        reject_first_schedule: bool,
        reject_credentials: bool,
        seen_tokens: Mutex<Vec<String>>,
    }

    fn field(body: &str, name: &str) -> String {
        let marker = format!("name=\"{name}\"\r\n\r\n");
        body.split(&marker)
            .nth(1)
            .and_then(|rest| rest.split("\r\n").next())
            .unwrap_or_default()
            .to_string()
    }

    async fn get_token(State(fake): State<Arc<Fake>>, body: String) -> String {
        let n = fake.token_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if fake.reject_credentials || field(&body, "password") != "secret-pw" {
            return r#"{"Authenticated":"False","UserToken":""}"#.into();
        }
        format!(r#"{{"Authenticated":"True","UserToken":"tok-{n}"}}"#)
    }

    async fn schedule(State(fake): State<Arc<Fake>>, body: String) -> String {
        let n = fake.schedule_calls.fetch_add(1, Ordering::SeqCst);
        fake.seen_tokens.lock().unwrap().push(field(&body, "token"));
        if fake.reject_first_schedule && n == 0 {
            return r#"{"errorMessage":"Invalid token."}"#.into();
        }
        include_str!("../../../fixtures/tracks/njt_newark.json").into()
    }

    async fn serve(fake: Arc<Fake>) -> String {
        let app = Router::new()
            .route("/TrainData/getToken", post(get_token))
            .route("/TrainData/getTrainSchedule19Rec", post(schedule))
            .with_state(fake);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}/TrainData")
    }

    fn token_path(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "raildata-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        dir.join("tracks").join("raildata-token.json")
    }

    fn client(base: &str, path: PathBuf, password: &str) -> RailDataClient {
        RailDataClient::new(
            reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            base,
            Some(("user".into(), password.into())),
            path,
        )
    }

    fn t(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_800_000_000 + secs)
    }

    #[tokio::test]
    async fn token_is_reused_and_survives_restart() {
        let fake = Arc::new(Fake::default());
        let base = serve(fake.clone()).await;
        let path = token_path("reuse");
        let mut first = client(&base, path.clone(), "secret-pw");
        first.schedule("NP", t(0)).await.unwrap();
        first.schedule("NP", t(60)).await.unwrap();
        let mut restarted = client(&base, path.clone(), "secret-pw");
        restarted.schedule("NP", t(120)).await.unwrap();
        assert_eq!(fake.token_calls.load(Ordering::SeqCst), 1);
        assert_eq!(*fake.seen_tokens.lock().unwrap(), vec!["tok-1"; 3]);

        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("secret-pw"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        // Past the reuse window a new token is requested.
        restarted
            .schedule("NP", t(TOKEN_REUSE.as_secs() + 1))
            .await
            .unwrap();
        assert_eq!(fake.token_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn rejected_token_gets_one_replacement() {
        let fake = Arc::new(Fake {
            reject_first_schedule: true,
            ..Fake::default()
        });
        let base = serve(fake.clone()).await;
        let mut raildata = client(&base, token_path("retry"), "secret-pw");
        let body = raildata.schedule("NP", t(0)).await.unwrap();
        assert!(body.contains("A67"));
        assert_eq!(fake.token_calls.load(Ordering::SeqCst), 2);
        assert_eq!(*fake.seen_tokens.lock().unwrap(), vec!["tok-1", "tok-2"]);
    }

    #[tokio::test]
    async fn budget_caps_token_requests_in_24_hours() {
        let fake = Arc::new(Fake::default());
        let base = serve(fake.clone()).await;
        let path = token_path("budget");
        let mut raildata = client(&base, path.clone(), "secret-pw");
        for hour in 0..TOKEN_BUDGET as u64 {
            // Force a fresh token each time by clearing the cached one.
            raildata.cache.clear_token();
            raildata.schedule("NP", t(hour * 3600)).await.unwrap();
        }
        raildata.cache.clear_token();
        assert_eq!(
            raildata.schedule("NP", t(10 * 3600)).await,
            Err(BoardError::BudgetExhausted)
        );
        // The budget also holds for a restarted process reading the same cache.
        let mut restarted = client(&base, path, "secret-pw");
        restarted.cache.clear_token();
        assert_eq!(
            restarted.schedule("NP", t(11 * 3600)).await,
            Err(BoardError::BudgetExhausted)
        );
        assert_eq!(fake.token_calls.load(Ordering::SeqCst), TOKEN_BUDGET);
        // Once the first request leaves the window, one more is allowed.
        restarted.schedule("NP", t(24 * 3600 + 1)).await.unwrap();
        assert_eq!(fake.token_calls.load(Ordering::SeqCst), TOKEN_BUDGET + 1);
    }

    #[tokio::test]
    async fn rejected_credentials_suspend_requests() {
        let fake = Arc::new(Fake::default());
        let base = serve(fake.clone()).await;
        let mut raildata = client(&base, token_path("creds"), "wrong-pw");
        assert_eq!(
            raildata.schedule("NP", t(0)).await,
            Err(BoardError::CredentialsRejected)
        );
        assert_eq!(
            raildata.schedule("NP", t(600)).await,
            Err(BoardError::Suspended)
        );
        assert_eq!(fake.token_calls.load(Ordering::SeqCst), 1);
        let error = format!("{:?}", raildata.schedule("NP", t(600)).await);
        assert!(!error.contains("wrong-pw"));
    }

    #[tokio::test]
    async fn missing_credentials_make_no_requests() {
        let fake = Arc::new(Fake::default());
        let base = serve(fake.clone()).await;
        let mut raildata =
            RailDataClient::new(reqwest::Client::new(), &base, None, token_path("none"));
        assert!(!raildata.is_configured());
        assert_eq!(
            raildata.schedule("NP", t(0)).await,
            Err(BoardError::Unconfigured)
        );
        assert_eq!(fake.token_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fake.schedule_calls.load(Ordering::SeqCst), 0);
    }
}
