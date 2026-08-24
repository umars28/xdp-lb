use aya::Pod;

pub const MAX_BACKENDS: u32 = 4096;
pub const MAX_SERVICES: u32 = 64;
pub const MAGLEV_SIZE: u32 = 4099;

pub const BACKEND_ACTIVE: u16 = 1 << 0;

pub const NO_BACKEND: u32 = u32::MAX;

pub const MODE_NAT: u8 = 0;

pub const STAT_NAMES: [&str; 7] = [
    "rx",
    "tx",
    "pass",
    "drop",
    "conntrack_hit",
    "conntrack_miss",
    "no_backend",
];

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

unsafe impl Pod for ServiceKey {}
unsafe impl Pod for ServiceInfo {}
unsafe impl Pod for Backend {}
unsafe impl Pod for StatVal {}
