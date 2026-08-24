use std::{net::Ipv4Addr, path::Path};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::types::{MAX_BACKENDS, MAX_SERVICES};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub interface: String,
    #[serde(default = "default_metrics_addr")]
    pub metrics_addr: String,
    #[serde(default = "default_health_interval")]
    pub health_interval_secs: u64,
    #[serde(default = "default_health_timeout")]
    pub health_timeout_ms: u64,
    pub services: Vec<ServiceConfig>,
}

#[derive(Debug, Deserialize)]
pub struct ServiceConfig {
    pub name: String,
    pub vip: Ipv4Addr,
    pub port: u16,
    #[serde(default = "default_protocol")]
    pub protocol: Protocol,
    pub backends: Vec<BackendConfig>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
}

impl Protocol {
    pub fn as_u8(self) -> u8 {
        match self {
            Protocol::Tcp => 6,
            Protocol::Udp => 17,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct BackendConfig {
    pub address: Ipv4Addr,
    pub port: u16,
    #[serde(default = "default_weight")]
    pub weight: u32,
    #[serde(default)]
    pub mac: Option<String>,
}

fn default_metrics_addr() -> String {
    "0.0.0.0:9500".to_string()
}

fn default_health_interval() -> u64 {
    3
}

fn default_health_timeout() -> u64 {
    500
}

fn default_protocol() -> Protocol {
    Protocol::Tcp
}

fn default_weight() -> u32 {
    1
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read config {}", path.display()))?;
        let config: Config = serde_yaml::from_str(&raw)
            .with_context(|| format!("cannot parse config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.services.is_empty() {
            bail!("config defines no services");
        }
        if self.services.len() as u32 > MAX_SERVICES {
            bail!(
                "{} services configured but datapath is built for {MAX_SERVICES}",
                self.services.len()
            );
        }

        let total: usize = self.services.iter().map(|s| s.backends.len()).sum();
        if total as u32 > MAX_BACKENDS {
            bail!("{total} backends configured but datapath is built for {MAX_BACKENDS}");
        }

        for svc in &self.services {
            if svc.backends.is_empty() {
                bail!("service {} has no backends", svc.name);
            }
            if svc.port == 0 {
                bail!("service {} has port 0", svc.name);
            }
            for be in &svc.backends {
                if be.port == 0 {
                    bail!("service {} has a backend with port 0", svc.name);
                }
                if be.weight == 0 {
                    bail!(
                        "service {} backend {}:{} has weight 0; remove it instead",
                        svc.name,
                        be.address,
                        be.port
                    );
                }
                if let Some(mac) = &be.mac {
                    parse_mac(mac).with_context(|| {
                        format!("service {} backend {}", svc.name, be.address)
                    })?;
                }
            }
        }

        Ok(())
    }
}

pub fn parse_mac(text: &str) -> Result<[u8; 6]> {
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() != 6 {
        bail!("malformed MAC address {text:?}");
    }
    let mut mac = [0u8; 6];
    for (slot, part) in mac.iter_mut().zip(parts) {
        *slot = u8::from_str_radix(part, 16)
            .with_context(|| format!("malformed MAC address {text:?}"))?;
    }
    Ok(mac)
}
