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
