//! Zero-dependency TOML + CLI configuration loader for `gserver`.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use gserver_player::{StarterHeroSeed, StarterItemSeed, StarterPackConfig};

const STARTER_HEROES: &[(u32, u8)] = &[(201, 2), (406, 3)];
const STARTER_ITEMS: &[(u32, u32)] = &[
    (16_012, 50),
    (22_102, 20),
    (22_202, 20),
    (101_101, 1),
    (112_001, 1),
    (180_101, 1),
    (182_001, 1),
    (50_101, 30),
    (3_100_101, 20),
    (3_113_001, 20),
    (17_004, 20),
];

/// Unified runtime configuration for both HTTP MAPI/Auth and TCP AMF3 Game Server listeners.
#[derive(Clone, Debug)]
pub struct DaemonConfig {
    pub http_bind: String,
    pub tcp_bind: String,
    pub public_host: String,
    pub data_dir: PathBuf,
    pub db_export_dir: Option<PathBuf>,
    pub bxh_snapshot_path: Option<PathBuf>,
    pub strict_session: bool,
    pub strict_sign: bool,
    pub strict_auth_sig: bool,
    pub strict_auth_token: bool,
    pub verify_tickets: bool,
    pub strict_rpc: bool,
    pub print_bound_addrs: bool,
    pub compress_s2c_threshold: usize,
    pub starter: StarterPackConfig,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        let starter = StarterPackConfig {
            initial_level: 10,
            initial_vip: 8,
            gold: 88_888,
            gold_bind: 50_000,
            copper: 10_000_000,
            execution: 300,
            energy: 150,
            server_open_time: "20261124080000".to_string(),
            default_group_id: "2900161".to_string(),
            server_id: 2_900_161,
            companion_heroes: STARTER_HEROES
                .iter()
                .map(|&(htid, slot)| StarterHeroSeed {
                    htid,
                    level: 10,
                    evolve_level: 1,
                    auto_deploy_slot: Some(slot),
                })
                .collect(),
            starter_items: STARTER_ITEMS
                .iter()
                .map(|&(item_template_id, count)| StarterItemSeed {
                    item_template_id,
                    count,
                })
                .collect(),
            ..StarterPackConfig::default()
        };

        Self {
            http_bind: "127.0.0.1:8080".to_string(),
            tcp_bind: "127.0.0.1:8160".to_string(),
            public_host: "127.0.0.1".to_string(),
            data_dir: PathBuf::from("./data"),
            db_export_dir: None,
            bxh_snapshot_path: Some(PathBuf::from("./official_bxh_snapshot.json")),
            strict_session: true,
            strict_sign: false,
            strict_auth_sig: false,
            strict_auth_token: false,
            verify_tickets: true,
            strict_rpc: false,
            print_bound_addrs: false,
            compress_s2c_threshold: 512,
            starter,
        }
    }
}

fn parse_into<T: FromStr>(val: &str, dst: &mut T) {
    if let Ok(parsed) = val.parse::<T>() {
        *dst = parsed;
    }
}

impl DaemonConfig {
    /// Constructs an ephemeral loopback configuration with every strict verification gate enabled.
    pub fn strict_all(data_dir: PathBuf) -> Self {
        let mut cfg = Self {
            http_bind: "127.0.0.1:0".to_string(),
            tcp_bind: "127.0.0.1:0".to_string(),
            public_host: "127.0.0.1".to_string(),
            data_dir,
            compress_s2c_threshold: 256,
            ..Self::default()
        };
        cfg.enable_strict_all();
        cfg
    }

    /// Enables strict HTTP signature, auth token, session cookie, login ticket, and RPC enforcement.
    pub fn enable_strict_all(&mut self) {
        self.strict_session = true;
        self.strict_sign = true;
        self.strict_auth_sig = true;
        self.strict_auth_token = true;
        self.verify_tickets = true;
        self.strict_rpc = true;
    }

    /// Parses a flat `key = value` TOML file into `self`.
    pub fn load_toml_file(&mut self, path: impl AsRef<Path>) -> io::Result<()> {
        let content = fs::read_to_string(path)?;
        self.apply_toml_str(&content);
        Ok(())
    }

    pub fn apply_toml_str(&mut self, content: &str) {
        for raw_line in content.lines() {
            let line = raw_line.split('#').next().unwrap_or("").trim();
            if line.is_empty() || line.starts_with('[') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let key = k.trim();
            let val = v.trim().trim_matches('"');
            let b = val == "true" || val == "1";
            match key {
                "http_bind" => self.http_bind = val.to_string(),
                "tcp_bind" => self.tcp_bind = val.to_string(),
                "public_host" => self.public_host = val.to_string(),
                "data_dir" => self.data_dir = PathBuf::from(val),
                "db_export_dir" if !val.is_empty() => self.db_export_dir = Some(PathBuf::from(val)),
                "bxh_snapshot_path" if !val.is_empty() => {
                    self.bxh_snapshot_path = Some(PathBuf::from(val));
                }
                "strict_session" => self.strict_session = b,
                "strict_sign" => self.strict_sign = b,
                "strict_auth_sig" => self.strict_auth_sig = b,
                "strict_auth_token" => self.strict_auth_token = b,
                "verify_tickets" => self.verify_tickets = b,
                "strict_rpc" => self.strict_rpc = b,
                "compress_s2c_threshold" => parse_into(val, &mut self.compress_s2c_threshold),
                "starter_level" => {
                    if let Ok(n) = val.parse::<u32>() {
                        self.starter.initial_level = n.max(1);
                    }
                }
                "starter_vip" => parse_into(val, &mut self.starter.initial_vip),
                "starter_gold" => parse_into(val, &mut self.starter.gold),
                "starter_gold_bind" => parse_into(val, &mut self.starter.gold_bind),
                "starter_copper" => parse_into(val, &mut self.starter.copper),
                "starter_execution" => parse_into(val, &mut self.starter.execution),
                "starter_energy" => parse_into(val, &mut self.starter.energy),
                "server_open_time" if val.len() >= 14 => {
                    self.starter.server_open_time = val.to_string();
                }
                _ => {}
            }
        }
    }

    fn parse_env_bool(val: &str) -> Option<bool> {
        match val.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        }
    }

    /// Applies `COBALT_*` environment variables from a provided map (used for deterministic tests
    /// and process environment bootstrap between `server.toml` and CLI flags).
    pub fn apply_env_map(&mut self, vars: &BTreeMap<String, String>) {
        let get_nonempty = |keys: &[&str]| -> Option<&str> {
            keys.iter()
                .filter_map(|k| vars.get(*k).map(|v| v.trim()))
                .find(|t| !t.is_empty())
        };

        for (keys, dst) in [
            (&["COBALT_HTTP_ADDR", "COBALT_HTTP_BIND"][..], &mut self.http_bind),
            (&["COBALT_TCP_ADDR", "COBALT_TCP_BIND"], &mut self.tcp_bind),
            (&["COBALT_PUBLIC_HOST"], &mut self.public_host),
        ] {
            if let Some(v) = get_nonempty(keys) {
                *dst = v.to_string();
            }
        }
        if let Some(v) = get_nonempty(&["COBALT_DATA_DIR"]) {
            self.data_dir = PathBuf::from(v);
        }
        for (key, dst) in [
            ("COBALT_DB_EXPORT_DIR", &mut self.db_export_dir),
            ("COBALT_BXH_SNAPSHOT", &mut self.bxh_snapshot_path),
        ] {
            if let Some(v) = get_nonempty(&[key]) {
                *dst = Some(PathBuf::from(v));
            }
        }
        if get_nonempty(&["COBALT_STRICT_ALL"]).and_then(Self::parse_env_bool) == Some(true) {
            self.enable_strict_all();
        }
        for (key, dst) in [
            ("COBALT_STRICT_SESSION", &mut self.strict_session),
            ("COBALT_STRICT_SIGN", &mut self.strict_sign),
            ("COBALT_STRICT_AUTH_SIG", &mut self.strict_auth_sig),
            ("COBALT_STRICT_AUTH_TOKEN", &mut self.strict_auth_token),
            ("COBALT_VERIFY_TICKETS", &mut self.verify_tickets),
            ("COBALT_STRICT_RPC", &mut self.strict_rpc),
            ("COBALT_PRINT_BOUND_ADDRS", &mut self.print_bound_addrs),
        ] {
            if let Some(b) = get_nonempty(&[key]).and_then(Self::parse_env_bool) {
                *dst = b;
            }
        }
    }

    /// Reads `COBALT_*` environment variables from the current process environment.
    pub fn apply_process_env(&mut self) {
        let vars: BTreeMap<String, String> = env::vars()
            .filter(|(k, _)| k.starts_with("COBALT_"))
            .collect();
        if !vars.is_empty() {
            self.apply_env_map(&vars);
        }
    }

    /// Parses CLI flags (`--config`, `COBALT_*` env vars, `--http-addr`, `--tcp-addr`, `--strict-all`, etc.).
    /// Precedence: `Default` < `server.toml` / `COBALT_CONFIG` / `--config` < `COBALT_*` env < CLI flags.
    pub fn from_cli_args<I, S>(args: I) -> Result<Option<Self>, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let vec: Vec<String> = args.into_iter().map(|s| s.as_ref().to_string()).collect();
        let mut cfg = Self::default();
        let next_arg = |idx: &mut usize, err: &str| -> Result<String, String> {
            *idx += 1;
            vec.get(*idx).cloned().ok_or_else(|| err.to_string())
        };

        let mut idx = 0;
        let mut loaded_explicit = false;
        while idx < vec.len() {
            match vec[idx].as_str() {
                "--help" | "-h" => return Ok(None),
                "--config" => {
                    let path = next_arg(&mut idx, "missing value for --config")?;
                    cfg.load_toml_file(&path)
                        .map_err(|e| format!("failed to read config '{path}': {e}"))?;
                    loaded_explicit = true;
                }
                _ => {}
            }
            idx += 1;
        }
        if !loaded_explicit {
            if let Ok(env_cfg) = env::var("COBALT_CONFIG") {
                let trimmed = env_cfg.trim();
                if !trimmed.is_empty() {
                    cfg.load_toml_file(trimmed)
                        .map_err(|e| format!("failed to read COBALT_CONFIG '{trimmed}': {e}"))?;
                    loaded_explicit = true;
                }
            }
        }
        if !loaded_explicit && Path::new("server.toml").exists() {
            let _ = cfg.load_toml_file("server.toml");
        }

        cfg.apply_process_env();

        idx = 0;
        while idx < vec.len() {
            match vec[idx].as_str() {
                "--config" => idx += 1,
                "--http-addr" | "--http-bind" => {
                    cfg.http_bind = next_arg(&mut idx, "missing --http-addr value")?;
                }
                "--tcp-addr" | "--tcp-bind" => {
                    cfg.tcp_bind = next_arg(&mut idx, "missing --tcp-addr value")?;
                }
                "--public-host" => {
                    cfg.public_host = next_arg(&mut idx, "missing --public-host value")?;
                }
                "--data-dir" => {
                    cfg.data_dir = PathBuf::from(next_arg(&mut idx, "missing --data-dir value")?);
                }
                "--db-export-dir" => {
                    cfg.db_export_dir = Some(PathBuf::from(next_arg(
                        &mut idx,
                        "missing --db-export-dir value",
                    )?));
                }
                "--strict-all" => cfg.enable_strict_all(),
                "--strict-sign" => cfg.strict_sign = true,
                "--strict-auth-sig" => cfg.strict_auth_sig = true,
                "--strict-auth-token" => cfg.strict_auth_token = true,
                "--strict-rpc" => cfg.strict_rpc = true,
                "--print-bound-addrs" => cfg.print_bound_addrs = true,
                "--no-strict-session" => cfg.strict_session = false,
                "--no-verify-tickets" => cfg.verify_tickets = false,
                other => return Err(format!("unknown CLI flag: {other}")),
            }
            idx += 1;
        }

        Ok(Some(cfg))
    }
}
