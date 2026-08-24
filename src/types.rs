use std::net::Ipv4Addr;

use aya::Pod;

pub const MAX_BACKENDS: u32 = 4096;
pub const MAX_SERVICES: u32 = 64;
pub const MAGLEV_SIZE: u32 = 4099;

pub const BACKEND_ACTIVE: u16 = 1 << 0;

pub const NO_BACKEND: u32 = u32::MAX;

pub const MODE_NAT: u8 = 0;
pub const MODE_DSR: u8 = 1;

pub const STAT_NAMES: [&str; 9] = [
    "rx",
    "tx",
    "pass",
    "drop",
    "conntrack_hit",
    "conntrack_miss",
    "no_backend",
    "rate_limited",
    "no_headroom",
];

pub const NANOS_PER_SECOND: u64 = 1_000_000_000;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ServiceKey {
    pub vip: u32,
    pub port: u16,
    pub proto: u8,
    pub pad: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ServiceInfo {
    pub svc_id: u32,
    pub dsr_source: u32,
    pub mode: u8,
    pub pad: [u8; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Backend {
    pub addr: u32,
    pub port: u16,
    pub flags: u16,
    pub mac: [u8; 6],
    pub pad: [u8; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct StatVal {
    pub packets: u64,
    pub bytes: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RateConfig {
    pub interval_ns: u64,
    pub burst: u64,
    pub enabled: u8,
    pub pad: [u8; 7],
}

impl RateConfig {
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn per_cpu(new_flows_per_second: u64, burst: u64) -> Self {
        Self {
            interval_ns: NANOS_PER_SECOND / new_flows_per_second.max(1),
            burst: burst.max(1),
            enabled: 1,
            pad: [0; 7],
        }
    }

    pub fn spread_across(cpus: u64, new_flows_per_second: u64, burst: u64) -> Self {
        let cpus = cpus.max(1);
        Self::per_cpu(new_flows_per_second / cpus, burst / cpus)
    }
}

unsafe impl Pod for ServiceKey {}
unsafe impl Pod for ServiceInfo {}
unsafe impl Pod for Backend {}
unsafe impl Pod for StatVal {}
unsafe impl Pod for RateConfig {}

pub fn be32(address: Ipv4Addr) -> u32 {
    u32::from_ne_bytes(address.octets())
}

pub fn be16(port: u16) -> u16 {
    u16::from_ne_bytes(port.to_be_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_is_written_in_network_order() {
        let value = be32("1.2.3.4".parse().unwrap());
        assert_eq!(value.to_ne_bytes(), [1, 2, 3, 4]);
    }

    #[test]
    fn port_is_written_in_network_order() {
        assert_eq!(be16(8080).to_ne_bytes(), [0x1f, 0x90]);
    }

    #[test]
    fn rate_config_turns_a_rate_into_a_token_interval() {
        let cfg = RateConfig::per_cpu(1000, 2000);
        assert_eq!(cfg.interval_ns, 1_000_000);
        assert_eq!(cfg.burst, 2000);
        assert_eq!(cfg.enabled, 1);
    }

    #[test]
    fn rate_config_never_produces_a_zero_interval() {
        let cfg = RateConfig::per_cpu(0, 0);
        assert!(cfg.interval_ns > 0, "a zero interval would divide by zero");
        assert!(cfg.burst > 0, "a zero burst would drop every packet");
    }

    #[test]
    fn a_rate_is_divided_across_the_per_cpu_buckets() {
        let cfg = RateConfig::spread_across(4, 1000, 2000);
        assert_eq!(cfg.interval_ns, 4_000_000, "250 flows per second per cpu");
        assert_eq!(cfg.burst, 500);
    }

    #[test]
    fn a_rate_below_one_per_cpu_still_admits_one() {
        let cfg = RateConfig::spread_across(64, 1, 1);
        assert_eq!(cfg.interval_ns, NANOS_PER_SECOND);
        assert_eq!(cfg.burst, 1);
    }

    #[test]
    fn rate_config_matches_the_datapath_layout() {
        assert_eq!(std::mem::size_of::<RateConfig>(), 24);
        assert_eq!(std::mem::align_of::<RateConfig>(), 8);
    }

    #[test]
    fn stat_names_cover_every_datapath_counter() {
        assert_eq!(STAT_NAMES.len(), 9);
    }

    #[test]
    fn service_info_matches_the_datapath_layout() {
        assert_eq!(std::mem::size_of::<ServiceInfo>(), 12);
        assert_eq!(std::mem::align_of::<ServiceInfo>(), 4);
    }

    #[test]
    fn service_key_matches_the_datapath_layout() {
        assert_eq!(std::mem::size_of::<ServiceKey>(), 8);
    }

    #[test]
    fn backend_matches_the_datapath_layout() {
        assert_eq!(std::mem::size_of::<Backend>(), 16);
    }
}
