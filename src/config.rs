use std::{net::Ipv4Addr, path::Path};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::types::{MAGLEV_SIZE, MAX_BACKENDS, MAX_SERVICES, MODE_DSR, MODE_NAT, NANOS_PER_SECOND};

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
    #[serde(default = "default_forwarding")]
    pub forwarding: Forwarding,
    #[serde(default)]
    pub dsr_source: Option<Ipv4Addr>,
    pub backends: Vec<BackendConfig>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Forwarding {
    Nat,
    Dsr,
}

impl Forwarding {
    pub fn as_u8(self) -> u8 {
        match self {
            Forwarding::Nat => MODE_NAT,
            Forwarding::Dsr => MODE_DSR,
        }
    }
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

fn default_forwarding() -> Forwarding {
    Forwarding::Nat
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
        Self::parse(&raw).with_context(|| format!("in config {}", path.display()))
    }

    pub fn parse(raw: &str) -> Result<Self> {
        let config: Config = serde_yaml::from_str(raw).context("cannot parse config")?;
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
            if svc.forwarding == Forwarding::Dsr && svc.dsr_source.is_none() {
                bail!(
                    "service {} uses dsr forwarding but has no dsr_source; the outer IPIP header \
                     needs a source address the backends can route back to",
                    svc.name
                );
            }
            if svc.forwarding == Forwarding::Nat && svc.dsr_source.is_some() {
                bail!(
                    "service {} sets dsr_source but forwards with nat; remove one of the two so \
                     the intent is unambiguous",
                    svc.name
                );
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
                if svc.forwarding == Forwarding::Dsr && be.port != svc.port {
                    bail!(
                        "service {} forwards with dsr to {}:{} but listens on port {}; dsr does not \
                         rewrite ports, so traffic would arrive on {} while health checks probe {}",
                        svc.name,
                        be.address,
                        be.port,
                        svc.port,
                        svc.port,
                        be.port
                    );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn service(extra: &str, backends: &str) -> String {
        format!(
            "interface: eth0\nservices:\n  - name: web\n    vip: 10.0.0.100\n    port: 80\n{extra}    backends:\n{backends}"
        )
    }

    fn one_backend(port: u16) -> String {
        format!("      - address: 10.0.0.11\n        port: {port}\n")
    }

    fn error_of(raw: &str) -> String {
        format!(
            "{:#}",
            Config::parse(raw).expect_err("config must be rejected")
        )
    }

    #[test]
    fn a_minimal_config_is_accepted() {
        let cfg = Config::parse(&service("", &one_backend(8080))).expect("must parse");
        assert_eq!(cfg.services[0].forwarding, Forwarding::Nat);
        assert_eq!(cfg.services[0].protocol, Protocol::Tcp);
        assert_eq!(cfg.services[0].backends[0].weight, 1);
        assert!(!cfg.services[0].backends[0].drain);
    }

    #[test]
    fn dsr_without_a_source_address_is_rejected() {
        let raw = service("    forwarding: dsr\n", &one_backend(80));
        assert!(error_of(&raw).contains("dsr_source"));
    }

    #[test]
    fn dsr_with_a_mismatched_backend_port_is_rejected() {
        let raw = service(
            "    forwarding: dsr\n    dsr_source: 10.0.0.1\n",
            &one_backend(8080),
        );
        let error = error_of(&raw);
        assert!(error.contains("does not rewrite ports"), "got: {error}");
    }

    #[test]
    fn dsr_with_a_matching_backend_port_is_accepted() {
        let raw = service(
            "    forwarding: dsr\n    dsr_source: 10.0.0.1\n",
            &one_backend(80),
        );
        let cfg = Config::parse(&raw).expect("must parse");
        assert_eq!(cfg.services[0].forwarding, Forwarding::Dsr);
    }

    #[test]
    fn nat_with_a_dsr_source_is_rejected_as_ambiguous() {
        let raw = service("    dsr_source: 10.0.0.1\n", &one_backend(8080));
        assert!(error_of(&raw).contains("unambiguous"));
    }

    #[test]
    fn weight_zero_is_rejected_because_it_hides_a_removal() {
        let raw = service(
            "",
            "      - address: 10.0.0.11\n        port: 8080\n        weight: 0\n",
        );
        assert!(error_of(&raw).contains("weight 0"));
    }

    #[test]
    fn a_service_without_backends_is_rejected() {
        let raw = "interface: eth0\nservices:\n  - name: web\n    vip: 10.0.0.100\n    port: 80\n    backends: []\n";
        assert!(error_of(raw).contains("no backends"));
    }

    #[test]
    fn weights_that_overflow_the_maglev_table_are_rejected() {
        let backends: String = (0..64)
            .map(|i| format!("      - address: 10.0.1.{i}\n        port: 8080\n"))
            .collect();
        let raw = service("", &backends);
        let with_weighting = raw.replace(
            "services:",
            "weighting:\n  endpoint: http://localhost:9090\n  query: up\n  max_weight: 128\nservices:",
        );
        let error = error_of(&with_weighting);
        assert!(error.contains("maglev candidates"), "got: {error}");
    }

    #[test]
    fn a_rate_limit_of_zero_is_rejected() {
        let raw = service("", &one_backend(8080)).replace(
            "services:",
            "rate_limit:\n  new_flows_per_second: 0\nservices:",
        );
        assert!(error_of(&raw).contains("drop every new connection"));
    }

    #[test]
    fn a_weighting_endpoint_without_a_scheme_is_rejected() {
        let raw = service("", &one_backend(8080)).replace(
            "services:",
            "weighting:\n  endpoint: localhost:9090\n  query: up\nservices:",
        );
        assert!(error_of(&raw).contains("http://"));
    }

    #[test]
    fn max_weight_below_min_weight_is_rejected() {
        let raw = service("", &one_backend(8080)).replace(
            "services:",
            "weighting:\n  endpoint: http://localhost:9090\n  query: up\n  min_weight: 8\n  max_weight: 4\nservices:",
        );
        assert!(error_of(&raw).contains("below min_weight"));
    }

    #[test]
    fn a_backend_metrics_identity_falls_back_to_its_address() {
        let cfg = Config::parse(&service("", &one_backend(8080))).expect("must parse");
        assert_eq!(cfg.services[0].backends[0].metrics_identity(), "10.0.0.11");
    }

    #[test]
    fn a_malformed_mac_is_rejected() {
        let raw = service(
            "",
            "      - address: 10.0.0.11\n        port: 8080\n        mac: \"nope\"\n",
        );
        assert!(error_of(&raw).contains("MAC"));
    }

    #[test]
    fn a_mac_is_parsed_from_colon_separated_hex() {
        assert_eq!(
            parse_mac("aa:bb:cc:00:11:22").unwrap(),
            [0xaa, 0xbb, 0xcc, 0x00, 0x11, 0x22]
        );
    }
}
