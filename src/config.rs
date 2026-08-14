use std::collections::BTreeSet;
use std::fmt;
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

const DEFAULT_STATIC_URL: &str = "https://content.amtrak.com/content/gtfs/GTFS.zip";
const DEFAULT_VALIDATOR_JAR: &str = "./tools/gtfs-validator-v8.0.1-cli.jar";
const REQUIRED_VALIDATOR_VERSION: &str = "8.0.1";
const OFFICIAL_VALIDATOR_SHA256: &str =
    "19293ddd9b6f954f216d4f12054bd8a3232921751c4484339e339764a91000e2";
const HARDENED_VALIDATOR_SHA256: &str =
    "24ca7e890ca15bfbb36fa889fcb16200f7276995b7e6ec75551a8b7175e818d7";
const MINIMUM_JAVA_MAJOR: u32 = 17;
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Operator-provided service configuration before safety validation.
///
/// Call [`Config::validate`] before constructing network listeners or refresh
/// tasks. The unvalidated form exists so parsing failures and unsafe exposure
/// combinations can identify the responsible environment field.
#[derive(Clone, Debug)]
pub struct Config {
    pub static_url: String,
    pub output_dir: PathBuf,
    pub poll_interval: Duration,
    pub static_refresh_interval: Duration,
    pub filter_capital_corridor: bool,
    pub bind_addr: SocketAddr,
    pub allowed_peer_ips: BTreeSet<IpAddr>,
    pub freshness_limit: Duration,
    pub gtfs_validator_jar: PathBuf,
}

/// Configuration proven safe enough to construct the service.
///
/// Validation guarantees a usable static URL, positive refresh intervals, a
/// provisioned pinned GTFS validator, and an explicit access policy for every
/// non-loopback listener.
#[derive(Clone, Debug)]
pub struct ValidatedConfig {
    pub static_url: String,
    pub output_dir: PathBuf,
    pub poll_interval: Duration,
    pub static_refresh_interval: Duration,
    pub filter_capital_corridor: bool,
    pub bind_addr: SocketAddr,
    pub freshness_limit: Duration,
    pub gtfs_validator_jar: PathBuf,
    pub access_policy: AccessPolicy,
}

/// Exact transport-peer policy for protected HTTP routes.
///
/// An empty allowlist admits loopback peers only. A non-empty allowlist admits
/// exactly its members; forwarded identity headers are outside this contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccessPolicy {
    allowed_peer_ips: BTreeSet<IpAddr>,
}

impl AccessPolicy {
    #[cfg(test)]
    pub(crate) fn from_allowed_ips_for_test(allowed_peer_ips: BTreeSet<IpAddr>) -> Self {
        Self { allowed_peer_ips }
    }
}

/// Result of evaluating a direct transport peer against [`AccessPolicy`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessDecision {
    Allow,
    Deny,
}

/// Field-specific configuration validation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigError {
    field: &'static str,
    reason: String,
}

impl ConfigError {
    fn new(field: &'static str, reason: impl Into<String>) -> Self {
        Self {
            field,
            reason: reason.into(),
        }
    }

    /// Returns the environment field responsible for this failure.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "binary error inspection API is exercised by tests"
        )
    )]
    pub fn field(&self) -> &'static str {
        self.field
    }

    /// Returns the actionable reason the field was rejected.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "binary error inspection API is exercised by tests"
        )
    )]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid {}: {}", self.field, self.reason)
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Parses service configuration from the process environment.
    ///
    /// # Errors
    ///
    /// Returns a field-specific string when an environment value cannot be
    /// parsed. Safety checks that require the complete configuration are
    /// performed by [`Config::validate`].
    pub fn from_env() -> Result<Config, ConfigError> {
        Config::from_map(|key| std::env::var(key).ok())
    }

    /// Parses service configuration from an injected environment lookup.
    ///
    /// This entry point keeps configuration parsing deterministic in tests and
    /// other embedding contexts.
    ///
    /// # Errors
    ///
    /// Returns a string naming the malformed environment field.
    pub fn from_map<F: Fn(&str) -> Option<String>>(get: F) -> Result<Config, ConfigError> {
        let static_url = get("AMTRAK_STATIC_URL").unwrap_or_else(|| DEFAULT_STATIC_URL.to_string());
        let output_dir = get("AMTRAK_OUTPUT_DIR")
            .unwrap_or_else(|| "./out".to_string())
            .into();
        let poll_secs = parse_u64(&get, "AMTRAK_POLL_SECS", 45)?;
        let static_refresh_secs = parse_u64(&get, "AMTRAK_STATIC_REFRESH_SECS", 86_400)?;
        let filter_capital_corridor = get("AMTRAK_FILTER_CAPITAL_CORRIDOR")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let bind_addr = get("AMTRAK_BIND_ADDR")
            .unwrap_or_else(|| "127.0.0.1:8080".to_string())
            .parse::<SocketAddr>()
            .map_err(|error| ConfigError::new("AMTRAK_BIND_ADDR", error.to_string()))?;
        let allowed_peer_ips = parse_peer_ips(get("AMTRAK_ALLOWED_PEER_IPS"))?;
        let gtfs_validator_jar = get("AMTRAK_GTFS_VALIDATOR_JAR")
            .unwrap_or_else(|| DEFAULT_VALIDATOR_JAR.to_string())
            .into();
        Ok(Config {
            static_url,
            output_dir,
            poll_interval: Duration::from_secs(poll_secs),
            static_refresh_interval: Duration::from_secs(static_refresh_secs),
            filter_capital_corridor,
            bind_addr,
            allowed_peer_ips,
            freshness_limit: Duration::from_secs(300),
            gtfs_validator_jar,
        })
    }

    /// Converts parsed settings into configuration safe for service startup.
    ///
    /// The default is loopback-only. Binding a non-loopback address requires a
    /// non-empty exact-IP allowlist, preventing an omitted policy from exposing
    /// feeds publicly. The validator probe confirms the pinned CLI artifact and
    /// Java 17 or newer before any feed can be accepted.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] naming the empty, unsafe, missing, unreadable,
    /// unpinned, or operationally unavailable field.
    pub fn validate(self) -> Result<ValidatedConfig, ConfigError> {
        self.validate_with_probe(probe_validator_runtime)
    }

    fn validate_with_probe<P>(self, probe: P) -> Result<ValidatedConfig, ConfigError>
    where
        P: FnOnce(&Path) -> Result<(), ConfigError>,
    {
        validate_static_url(&self.static_url)?;
        if self.output_dir.as_os_str().is_empty() {
            return Err(ConfigError::new("AMTRAK_OUTPUT_DIR", "must not be empty"));
        }
        if self.poll_interval.is_zero() {
            return Err(ConfigError::new(
                "AMTRAK_POLL_SECS",
                "must be greater than zero",
            ));
        }
        if self.static_refresh_interval.is_zero() {
            return Err(ConfigError::new(
                "AMTRAK_STATIC_REFRESH_SECS",
                "must be greater than zero",
            ));
        }
        if self.freshness_limit != Duration::from_secs(300) {
            return Err(ConfigError::new(
                "freshness_limit",
                "must remain fixed at 300 seconds for this service contract",
            ));
        }
        if !self.bind_addr.ip().is_loopback() && self.allowed_peer_ips.is_empty() {
            return Err(ConfigError::new(
                "AMTRAK_ALLOWED_PEER_IPS",
                "is required when AMTRAK_BIND_ADDR is not loopback",
            ));
        }
        probe(&self.gtfs_validator_jar)?;

        Ok(ValidatedConfig {
            static_url: self.static_url,
            output_dir: self.output_dir,
            poll_interval: self.poll_interval,
            static_refresh_interval: self.static_refresh_interval,
            filter_capital_corridor: self.filter_capital_corridor,
            bind_addr: self.bind_addr,
            freshness_limit: self.freshness_limit,
            gtfs_validator_jar: self.gtfs_validator_jar,
            access_policy: AccessPolicy {
                allowed_peer_ips: self.allowed_peer_ips,
            },
        })
    }
}

/// Evaluates the direct socket peer for a protected route.
///
/// Forwarding headers are deliberately absent from this interface. An empty
/// policy admits loopback peers, while a configured policy admits only exact
/// addresses.
pub fn authorize(policy: &AccessPolicy, peer: IpAddr) -> AccessDecision {
    let allowed = if policy.allowed_peer_ips.is_empty() {
        peer.is_loopback()
    } else {
        policy.allowed_peer_ips.contains(&peer)
    };
    if allowed {
        AccessDecision::Allow
    } else {
        AccessDecision::Deny
    }
}

fn validate_static_url(value: &str) -> Result<(), ConfigError> {
    let parsed = reqwest::Url::parse(value)
        .map_err(|error| ConfigError::new("AMTRAK_STATIC_URL", error.to_string()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ConfigError::new(
            "AMTRAK_STATIC_URL",
            "scheme must be http or https",
        ));
    }
    if parsed.host_str().is_none() {
        return Err(ConfigError::new("AMTRAK_STATIC_URL", "must include a host"));
    }
    Ok(())
}

fn parse_peer_ips(value: Option<String>) -> Result<BTreeSet<IpAddr>, ConfigError> {
    let Some(value) = value else {
        return Ok(BTreeSet::new());
    };
    if value.trim().is_empty() {
        return Ok(BTreeSet::new());
    }
    value
        .split(',')
        .map(|entry| {
            let entry = entry.trim();
            entry.parse().map_err(|error| {
                ConfigError::new(
                    "AMTRAK_ALLOWED_PEER_IPS",
                    format!("entry {entry:?} is invalid: {error}"),
                )
            })
        })
        .collect()
}

fn probe_validator_runtime(path: &Path) -> Result<(), ConfigError> {
    let digest = sha256_file(path).map_err(|error| {
        ConfigError::new(
            "AMTRAK_GTFS_VALIDATOR_JAR",
            format!("{} is not a readable file: {error}", path.display()),
        )
    })?;
    if !validator_digest_is_approved(&digest) {
        return Err(ConfigError::new(
            "AMTRAK_GTFS_VALIDATOR_JAR",
            format!("must match an approved MobilityData {REQUIRED_VALIDATOR_VERSION} CLI SHA-256"),
        ));
    }

    let output = run_with_timeout(Command::new("java").arg("--version"), PROBE_TIMEOUT, true)
        .map_err(|reason| ConfigError::new("AMTRAK_GTFS_VALIDATOR_JAR", reason))?;
    let version_output = String::from_utf8_lossy(if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    });
    let java_major = parse_java_major(&version_output).ok_or_else(|| {
        ConfigError::new(
            "AMTRAK_GTFS_VALIDATOR_JAR",
            "could not determine the Java runtime version",
        )
    })?;
    if java_major < MINIMUM_JAVA_MAJOR {
        return Err(ConfigError::new(
            "AMTRAK_GTFS_VALIDATOR_JAR",
            format!("requires Java {MINIMUM_JAVA_MAJOR} or newer; found {java_major}"),
        ));
    }
    run_with_timeout(
        Command::new("java").arg("-jar").arg(path).arg("--help"),
        PROBE_TIMEOUT,
        false,
    )
    .map_err(|reason| ConfigError::new("AMTRAK_GTFS_VALIDATOR_JAR", reason))?;
    Ok(())
}

fn validator_digest_is_approved(digest: &str) -> bool {
    digest == OFFICIAL_VALIDATOR_SHA256 || digest == HARDENED_VALIDATOR_SHA256
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[derive(Debug)]
struct CommandOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_with_timeout(
    command: &mut Command,
    timeout: Duration,
    capture_output: bool,
) -> Result<CommandOutput, String> {
    if capture_output {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    } else {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .spawn()
        .map_err(|error| format!("{program} could not be started: {error}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child
                    .wait_with_output()
                    .map_err(|error| format!("{program} output could not be read: {error}"))?;
                if !status.success() {
                    return Err(format!("{program} exited unsuccessfully"));
                }
                return Ok(CommandOutput {
                    stdout: output.stdout,
                    stderr: output.stderr,
                });
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{program} exceeded the {} second timeout",
                    timeout.as_secs()
                ));
            }
            Err(error) => return Err(format!("{program} status could not be read: {error}")),
        }
    }
}

fn parse_java_major(output: &str) -> Option<u32> {
    output
        .split(|character: char| !character.is_ascii_digit() && character != '.')
        .find(|token| {
            token
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_digit())
        })
        .and_then(|version| version.split('.').next())
        .and_then(|major| major.parse().ok())
}

fn parse_u64<F: Fn(&str) -> Option<String>>(
    get: &F,
    key: &str,
    default: u64,
) -> Result<u64, ConfigError> {
    match get(key) {
        Some(value) => value
            .parse::<u64>()
            .map_err(|error| ConfigError::new(env_field(key), error.to_string())),
        None => Ok(default),
    }
}

fn env_field(key: &str) -> &'static str {
    match key {
        "AMTRAK_POLL_SECS" => "AMTRAK_POLL_SECS",
        "AMTRAK_STATIC_REFRESH_SECS" => "AMTRAK_STATIC_REFRESH_SECS",
        _ => "configuration",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn map(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let values: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        move |key: &str| values.get(key).cloned()
    }

    #[test]
    fn defaults_apply_when_env_absent() {
        let config = Config::from_map(map(&[])).unwrap();
        assert_eq!(config.static_url, DEFAULT_STATIC_URL);
        assert_eq!(config.output_dir, PathBuf::from("./out"));
        assert_eq!(config.poll_interval, Duration::from_secs(45));
        assert_eq!(config.static_refresh_interval, Duration::from_secs(86_400));
        assert!(!config.filter_capital_corridor);
        assert_eq!(config.bind_addr, "127.0.0.1:8080".parse().unwrap());
        assert!(config.allowed_peer_ips.is_empty());
        assert_eq!(config.freshness_limit, Duration::from_secs(300));
        assert_eq!(
            config.gtfs_validator_jar,
            PathBuf::from(DEFAULT_VALIDATOR_JAR)
        );
    }

    #[test]
    fn env_overrides_apply() {
        let config = Config::from_map(map(&[
            ("AMTRAK_POLL_SECS", "10"),
            ("AMTRAK_FILTER_CAPITAL_CORRIDOR", "true"),
            ("AMTRAK_BIND_ADDR", "127.0.0.1:9000"),
            ("AMTRAK_ALLOWED_PEER_IPS", "127.0.0.1, 10.0.0.4"),
            (
                "AMTRAK_GTFS_VALIDATOR_JAR",
                "/opt/gtfs-validator-v8.0.1-cli.jar",
            ),
        ]))
        .unwrap();
        assert_eq!(config.poll_interval, Duration::from_secs(10));
        assert!(config.filter_capital_corridor);
        assert_eq!(config.bind_addr, "127.0.0.1:9000".parse().unwrap());
        assert_eq!(config.allowed_peer_ips.len(), 2);
        assert_eq!(
            config.gtfs_validator_jar,
            PathBuf::from("/opt/gtfs-validator-v8.0.1-cli.jar")
        );
    }

    #[test]
    fn invalid_number_errors() {
        let error = Config::from_map(map(&[("AMTRAK_POLL_SECS", "abc")])).unwrap_err();
        assert_eq!(error.field(), "AMTRAK_POLL_SECS");
    }

    fn validated(pairs: &[(&str, &str)]) -> Result<ValidatedConfig, ConfigError> {
        Config::from_map(map(pairs))
            .unwrap()
            .validate_with_probe(|_| Ok(()))
    }

    #[test]
    fn validation_accepts_loopback_default_and_exact_non_loopback_policy() {
        let local = validated(&[]).unwrap();
        assert_eq!(local.static_url, DEFAULT_STATIC_URL);
        assert_eq!(local.output_dir, PathBuf::from("./out"));
        assert_eq!(local.poll_interval, Duration::from_secs(45));
        assert_eq!(local.static_refresh_interval, Duration::from_secs(86_400));
        assert!(!local.filter_capital_corridor);
        assert_eq!(local.bind_addr, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(local.freshness_limit, Duration::from_secs(300));
        assert_eq!(
            local.gtfs_validator_jar,
            PathBuf::from(DEFAULT_VALIDATOR_JAR)
        );
        assert_eq!(
            authorize(&local.access_policy, "127.0.0.1".parse().unwrap()),
            AccessDecision::Allow
        );
        assert_eq!(
            authorize(&local.access_policy, "10.0.0.4".parse().unwrap()),
            AccessDecision::Deny
        );

        let internal = validated(&[
            ("AMTRAK_BIND_ADDR", "0.0.0.0:8080"),
            ("AMTRAK_ALLOWED_PEER_IPS", "10.0.0.4"),
        ])
        .unwrap();
        assert_eq!(
            authorize(&internal.access_policy, "10.0.0.4".parse().unwrap()),
            AccessDecision::Allow
        );
        assert_eq!(
            authorize(&internal.access_policy, "127.0.0.1".parse().unwrap()),
            AccessDecision::Deny
        );
    }

    #[test]
    fn validation_rejects_unsafe_and_malformed_fields_with_the_field_name() {
        let unsafe_bind = validated(&[("AMTRAK_BIND_ADDR", "0.0.0.0:8080")]).unwrap_err();
        assert_eq!(unsafe_bind.field(), "AMTRAK_ALLOWED_PEER_IPS");

        let invalid_url = validated(&[("AMTRAK_STATIC_URL", "not a URL")]).unwrap_err();
        assert_eq!(invalid_url.field(), "AMTRAK_STATIC_URL");

        let zero_poll = validated(&[("AMTRAK_POLL_SECS", "0")]).unwrap_err();
        assert_eq!(zero_poll.field(), "AMTRAK_POLL_SECS");

        let peers =
            Config::from_map(map(&[("AMTRAK_ALLOWED_PEER_IPS", "10.0.0.4,nope")])).unwrap_err();
        assert_eq!(peers.field(), "AMTRAK_ALLOWED_PEER_IPS");
    }

    #[test]
    fn validation_propagates_validator_probe_failures() {
        let error = Config::from_map(map(&[]))
            .unwrap()
            .validate_with_probe(|_| {
                Err(ConfigError::new(
                    "AMTRAK_GTFS_VALIDATOR_JAR",
                    "Java runtime is unavailable",
                ))
            })
            .unwrap_err();
        assert_eq!(error.field(), "AMTRAK_GTFS_VALIDATOR_JAR");
        assert!(error.reason().contains("Java"));
    }

    #[test]
    fn production_validation_rejects_missing_validator_artifact() {
        let error = Config::from_map(map(&[(
            "AMTRAK_GTFS_VALIDATOR_JAR",
            "/definitely/missing/gtfs-validator-v8.0.1-cli.jar",
        )]))
        .unwrap()
        .validate()
        .unwrap_err();
        assert_eq!(error.field(), "AMTRAK_GTFS_VALIDATOR_JAR");
        assert!(error.reason().contains("readable file"));
    }

    #[test]
    fn production_validation_rejects_wrong_validator_bytes_even_with_pinned_name() {
        let path = std::env::temp_dir().join(format!(
            "gtfs-validator-8.0.1-cli-{}.jar",
            std::process::id()
        ));
        std::fs::write(&path, b"not a jar").unwrap();
        let error = Config::from_map(map(&[(
            "AMTRAK_GTFS_VALIDATOR_JAR",
            path.to_str().unwrap(),
        )]))
        .unwrap()
        .validate()
        .unwrap_err();
        std::fs::remove_file(path).unwrap();
        assert_eq!(error.field(), "AMTRAK_GTFS_VALIDATOR_JAR");
        assert!(error.reason().contains("SHA-256"));
    }

    #[test]
    fn java_major_parser_accepts_documented_version_formats() {
        assert_eq!(parse_java_major("openjdk 17.0.12 2024-07-16"), Some(17));
        assert_eq!(parse_java_major("java 21.0.5 2024-10-15 LTS"), Some(21));
        assert_eq!(parse_java_major("unparseable"), None);
    }

    #[test]
    fn sha256_file_hashes_bytes_without_external_utilities() {
        let path = std::env::temp_dir().join(format!("sha256-file-{}", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        let digest = sha256_file(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn official_and_hardened_validator_digests_are_approved() {
        assert!(validator_digest_is_approved(OFFICIAL_VALIDATOR_SHA256));
        assert!(validator_digest_is_approved(HARDENED_VALIDATOR_SHA256));
        assert!(!validator_digest_is_approved(&"0".repeat(64)));
    }

    #[test]
    fn subprocess_probe_is_bounded() {
        let error = run_with_timeout(
            Command::new("sh").args(["-c", "sleep 1"]),
            Duration::from_millis(10),
            false,
        )
        .unwrap_err();
        assert!(error.contains("timeout"));
    }
}
