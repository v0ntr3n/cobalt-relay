//! Zero-dependency TOML + CLI configuration loader for `gserver`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use gserver_player::{StarterHeroSeed, StarterItemSeed, StarterPackConfig};

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
            server_id: 2900161,
            companion_heroes: vec![
                StarterHeroSeed {
                    htid: 201,
                    level: 10,
                    evolve_level: 1,
                    auto_deploy_slot: Some(2),
                },
                StarterHeroSeed {
                    htid: 406,
                    level: 10,
                    evolve_level: 1,
                    auto_deploy_slot: Some(3),
                },
            ],
            starter_items: vec![
                StarterItemSeed {
                    item_template_id: 16012,
                    count: 50,
                },
                StarterItemSeed {
                    item_template_id: 22102,
                    count: 20,
                },
                StarterItemSeed {
                    item_template_id: 22202,
                    count: 20,
                },
                StarterItemSeed {
                    item_template_id: 112_001,
                    count: 1,
                },
                StarterItemSeed {
                    item_template_id: 182_001,
                    count: 1,
                },
                StarterItemSeed {
                    item_template_id: 50_101,
                    count: 160,
                },
                StarterItemSeed {
                    item_template_id: 3_113_001,
                    count: 20,
                },
                StarterItemSeed {
                    item_template_id: 17_004,
                    count: 20,
                },
            ],
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
            match key {
                "http_bind" => self.http_bind = val.to_string(),
                "tcp_bind" => self.tcp_bind = val.to_string(),
                "public_host" => self.public_host = val.to_string(),
                "data_dir" => self.data_dir = PathBuf::from(val),
                "db_export_dir" if !val.is_empty() => {
                    self.db_export_dir = Some(PathBuf::from(val));
                }
                "bxh_snapshot_path" if !val.is_empty() => {
                    self.bxh_snapshot_path = Some(PathBuf::from(val));
                }
                "strict_session" => self.strict_session = val == "true" || val == "1",
                "strict_sign" => self.strict_sign = val == "true" || val == "1",
                "strict_auth_sig" => self.strict_auth_sig = val == "true" || val == "1",
                "strict_auth_token" => self.strict_auth_token = val == "true" || val == "1",
                "verify_tickets" => self.verify_tickets = val == "true" || val == "1",
                "strict_rpc" => self.strict_rpc = val == "true" || val == "1",
                "compress_s2c_threshold" => {
                    if let Ok(n) = val.parse::<usize>() {
                        self.compress_s2c_threshold = n;
                    }
                }
                "starter_level" => {
                    if let Ok(n) = val.parse::<u32>() {
                        self.starter.initial_level = n.max(1);
                    }
                }
                "starter_vip" => {
                    if let Ok(n) = val.parse::<u32>() {
                        self.starter.initial_vip = n;
                    }
                }
                "starter_gold" => {
                    if let Ok(n) = val.parse::<u64>() {
                        self.starter.gold = n;
                    }
                }
                "starter_gold_bind" => {
                    if let Ok(n) = val.parse::<u64>() {
                        self.starter.gold_bind = n;
                    }
                }
                "starter_copper" => {
                    if let Ok(n) = val.parse::<u64>() {
                        self.starter.copper = n;
                    }
                }
                "starter_execution" => {
                    if let Ok(n) = val.parse::<u64>() {
                        self.starter.execution = n;
                    }
                }
                "starter_energy" => {
                    if let Ok(n) = val.parse::<u64>() {
                        self.starter.energy = n;
                    }
                }
                "server_open_time" if val.len() >= 14 => {
                    self.starter.server_open_time = val.to_string();
                }
                _ => {}
            }
        }
    }

    /// Parses CLI flags (`--config`, `--http-addr`, `--tcp-addr`, `--strict-all`, etc.).
    pub fn from_cli_args<I, S>(args: I) -> Result<Option<Self>, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let vec: Vec<String> = args.into_iter().map(|s| s.as_ref().to_string()).collect();
        let mut cfg = Self::default();

        // First pass: check --config if provided, or auto-load server.toml if present in CWD
        let mut idx = 0;
        let mut loaded_explicit = false;
        while idx < vec.len() {
            if vec[idx] == "--help" || vec[idx] == "-h" {
                return Ok(None);
            }
            if vec[idx] == "--config" {
                let path = vec
                    .get(idx + 1)
                    .ok_or_else(|| "missing value for --config".to_string())?;
                cfg.load_toml_file(path)
                    .map_err(|e| format!("failed to read config '{path}': {e}"))?;
                loaded_explicit = true;
                idx += 2;
                continue;
            }
            idx += 1;
        }
        if !loaded_explicit && Path::new("server.toml").exists() {
            let _ = cfg.load_toml_file("server.toml");
        }

        // Second pass: apply CLI overrides
        idx = 0;
        while idx < vec.len() {
            match vec[idx].as_str() {
                "--config" => idx += 2,
                "--http-addr" | "--http-bind" => {
                    cfg.http_bind = vec
                        .get(idx + 1)
                        .ok_or("missing --http-addr value")?
                        .clone();
                    idx += 2;
                }
                "--tcp-addr" | "--tcp-bind" => {
                    cfg.tcp_bind = vec
                        .get(idx + 1)
                        .ok_or("missing --tcp-addr value")?
                        .clone();
                    idx += 2;
                }
                "--public-host" => {
                    cfg.public_host = vec
                        .get(idx + 1)
                        .ok_or("missing --public-host value")?
                        .clone();
                    idx += 2;
                }
                "--data-dir" => {
                    cfg.data_dir = PathBuf::from(
                        vec.get(idx + 1).ok_or("missing --data-dir value")?,
                    );
                    idx += 2;
                }
                "--db-export-dir" => {
                    cfg.db_export_dir = Some(PathBuf::from(
                        vec.get(idx + 1).ok_or("missing --db-export-dir value")?,
                    ));
                    idx += 2;
                }
                "--strict-all" => {
                    cfg.enable_strict_all();
                    idx += 1;
                }
                "--strict-sign" => {
                    cfg.strict_sign = true;
                    idx += 1;
                }
                "--strict-auth-sig" => {
                    cfg.strict_auth_sig = true;
                    idx += 1;
                }
                "--strict-auth-token" => {
                    cfg.strict_auth_token = true;
                    idx += 1;
                }
                "--strict-rpc" => {
                    cfg.strict_rpc = true;
                    idx += 1;
                }
                "--print-bound-addrs" => {
                    cfg.print_bound_addrs = true;
                    idx += 1;
                }
                "--no-strict-session" => {
                    cfg.strict_session = false;
                    idx += 1;
                }
                "--no-verify-tickets" => {
                    cfg.verify_tickets = false;
                    idx += 1;
                }
                other => return Err(format!("unknown CLI flag: {other}")),
            }
        }

        Ok(Some(cfg))
    }
}
