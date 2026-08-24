use std::{net::Ipv4Addr, path::Path};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::types::{MAGLEV_SIZE, MAX_BACKENDS, MAX_SERVICES, NANOS_PER_SECOND};

const GOOD_DISTRIBUTION_RATIO: u32 = 8;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub interface: String,
    #[serde(default = "default_metrics_addr")]
    pub metrics_addr: String,
    #[serde(default = "default_health_interval")]
    pub health_interval_secs: u64,
    #[serde(default = "default_health_timeout")]
    pub health_timeout_ms: u64,
    #[serde(default)]
    pub weighting: Option<WeightingConfig>,
    #[serde(default)]
    pub rate_limit: Option<RateLimitConfig>,
    pub services: Vec<ServiceConfig>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct RateLimitConfig {
    pub new_flows_per_second: u64,
    #[serde(default)]
    pub burst: Option<u64>,
}

impl RateLimitConfig {
    pub fn burst_or_default(&self) -> u64 {
        self.burst.unwrap_or(self.new_flows_per_second * 2)
    }

    fn validate(&self) -> Result<()> {
        if self.new_flows_per_second == 0 {
            bail!(
                "rate_limit.new_flows_per_second is 0, which would drop every new connection; \
                 remove the rate_limit block to disable it instead"
            );
        }
        if self.new_flows_per_second > NANOS_PER_SECOND {
            bail!(
                "rate_limit.new_flows_per_second ({}) exceeds one flow per nanosecond",
                self.new_flows_per_second
            );
        }
        if self.burst_or_default() == 0 {
            bail!("rate_limit.burst is 0, which would drop every new connection");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct WeightingConfig {
    pub endpoint: String,
    pub query: String,
    #[serde(default = "default_weight_mode")]
    pub mode: WeightMode,
    #[serde(default = "default_instance_label")]
    pub instance_label: String,
    #[serde(default = "default_weight_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_weight_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "default_min_weight")]
    pub min_weight: u32,
    #[serde(default = "default_max_weight")]
    pub max_weight: u32,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WeightMode {
    Proportional,
    Inverse,
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
    #[serde(default)]
    pub drain: bool,
    #[serde(default)]
    pub metrics_instance: Option<String>,
}

impl BackendConfig {
    pub fn key(&self) -> String {
        format!("{}:{}", self.address, self.port)
    }

    pub fn metrics_identity(&self) -> String {
        self.metrics_instance
            .clone()
            .unwrap_or_else(|| self.address.to_string())
    }
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

fn default_weight_mode() -> WeightMode {
    WeightMode::Proportional
}

fn default_instance_label() -> String {
    "instance".to_string()
}

fn default_weight_interval() -> u64 {
    15
}

fn default_weight_timeout() -> u64 {
    2000
}

fn default_min_weight() -> u32 {
    1
}

fn default_max_weight() -> u32 {
    16
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
                    parse_mac(mac)
                        .with_context(|| format!("service {} backend {}", svc.name, be.address))?;
                }
            }

            self.check_maglev_budget(svc)?;
        }

        if let Some(weighting) = &self.weighting {
            weighting.validate()?;
        }

        if let Some(rate_limit) = &self.rate_limit {
            rate_limit.validate()?;
        }

        Ok(())
    }

    fn check_maglev_budget(&self, svc: &ServiceConfig) -> Result<()> {
        let ceiling = match &self.weighting {
            Some(weighting) => weighting.max_weight,
            None => svc.backends.iter().map(|be| be.weight).max().unwrap_or(1),
        };
        let worst_case = svc.backends.len() as u32 * ceiling;

        if worst_case > MAGLEV_SIZE {
            bail!(
                "service {} can reach {worst_case} maglev candidates ({} backends x weight {ceiling}) \
                 but the table only has {MAGLEV_SIZE} slots; lower max_weight or split the service",
                svc.name,
                svc.backends.len()
            );
        }

        if worst_case * GOOD_DISTRIBUTION_RATIO > MAGLEV_SIZE {
            tracing::warn!(
                service = %svc.name,
                candidates = worst_case,
                slots = MAGLEV_SIZE,
                "maglev candidates are close to the table size; traffic share will drift from the configured weights"
            );
        }

        Ok(())
    }
}

impl WeightingConfig {
    fn validate(&self) -> Result<()> {
        if self.min_weight == 0 {
            bail!("weighting.min_weight must be at least 1; weight 0 removes a backend silently");
        }
        if self.max_weight < self.min_weight {
            bail!(
                "weighting.max_weight ({}) is below min_weight ({})",
                self.max_weight,
                self.min_weight
            );
        }
        if self.query.trim().is_empty() {
            bail!("weighting.query is empty");
        }
        if !self.endpoint.starts_with("http://") && !self.endpoint.starts_with("https://") {
            bail!(
                "weighting.endpoint must start with http:// or https://, got {:?}",
                self.endpoint
            );
        }
        if self.interval_secs == 0 {
            bail!("weighting.interval_secs must be at least 1");
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
