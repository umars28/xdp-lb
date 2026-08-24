use std::{collections::HashMap, net::Ipv4Addr, path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use aya::{programs::Xdp, programs::XdpFlags, Ebpf};
use clap::{Parser, ValueEnum};
use tokio::task::JoinSet;
use tracing::{info, warn};

use xdp_lb::{
    config::{self, Config, Forwarding, Protocol},
    dataplane::DataPlane,
    drain::DrainList,
    health, maglev,
    metrics::{self, AppState, BackendSample, SharedSnapshot},
    neigh, object,
    types::{
        be16, be32, Backend, RateConfig, ServiceInfo, ServiceKey, BACKEND_ACTIVE, MAGLEV_SIZE,
        NO_BACKEND,
    },
    weights::PrometheusSource,
};

#[derive(Parser, Debug)]
#[command(name = "xdp-lb", about = "L4 load balancer on the XDP hook")]
struct Cli {
    #[arg(short, long, default_value = "config.yaml")]
    config: PathBuf,

    #[arg(long)]
    interface: Option<String>,

    #[arg(long, value_enum, default_value_t = Mode::Skb)]
    xdp_mode: Mode,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Mode {
    Skb,
    Driver,
    Hardware,
}

impl Mode {
    fn flags(self) -> XdpFlags {
        match self {
            Mode::Skb => XdpFlags::SKB_MODE,
            Mode::Driver => XdpFlags::DRV_MODE,
            Mode::Hardware => XdpFlags::HW_MODE,
        }
    }
}

struct BackendSlot {
    index: u32,
    address: Ipv4Addr,
    port: u16,
    weight: u32,
    metrics_identity: String,
    mac_override: Option<[u8; 6]>,
    mac: Option<[u8; 6]>,
    healthy: bool,
}

impl BackendSlot {
    fn key(&self) -> String {
        format!("{}:{}", self.address, self.port)
    }
}

struct ServiceSlot {
    svc_id: u32,
    name: String,
    vip: Ipv4Addr,
    port: u16,
    proto: Protocol,
    forwarding: Forwarding,
    dsr_source: Option<Ipv4Addr>,
    backends: Vec<BackendSlot>,
    table: Vec<u32>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let cfg = Config::load(&cli.config)?;
    let interface = cli
        .interface
        .clone()
        .unwrap_or_else(|| cfg.interface.clone());

    bump_memlock()?;

    let mut ebpf = Ebpf::load(object::bytes()).context("loading the BPF object into the kernel")?;

    let program: &mut Xdp = ebpf
        .program_mut("xdp_lb")
        .context("BPF object has no program named xdp_lb")?
        .try_into()?;
    program.load().context("verifier rejected xdp_lb")?;
    program
        .attach(&interface, cli.xdp_mode.flags())
        .with_context(|| format!("attaching xdp_lb to {interface}"))?;
    info!(interface, mode = ?cli.xdp_mode, "xdp program attached");

    let mut plane = DataPlane::attach_maps(&mut ebpf)?;
    let mut slots = build_slots(&cfg);
    publish_services(&mut plane, &slots)?;

    let rate_config = match &cfg.rate_limit {
        Some(limit) => {
            let cpus = aya::util::nr_cpus().unwrap_or(1) as u64;
            let config = RateConfig::spread_across(
                cpus,
                limit.new_flows_per_second,
                limit.burst_or_default(),
            );
            info!(
                new_flows_per_second = limit.new_flows_per_second,
                burst = limit.burst_or_default(),
                cpus,
                per_cpu_interval_ns = config.interval_ns,
                per_cpu_burst = config.burst,
                "rate limiting new flows per source address"
            );
            config
        }
        None => RateConfig::disabled(),
    };
    plane.put_rate_config(rate_config)?;

    let drain = DrainList::seed(
        cfg.services
            .iter()
            .flat_map(|svc| svc.backends.iter())
            .filter(|be| be.drain)
            .map(|be| be.key()),
    );
    for entry in drain.entries() {
        info!(backend = %entry, "backend starts drained");
    }

    let snapshot = metrics::shared();
    let metrics_addr = cfg.metrics_addr.clone();
    let state = AppState {
        snapshot: snapshot.clone(),
        drain: drain.clone(),
    };
    tokio::spawn(async move {
        if let Err(err) = metrics::serve(&metrics_addr, state).await {
            warn!(%err, "metrics server exited");
        }
    });
    info!(addr = %cfg.metrics_addr, "metrics endpoint listening");

    let weight_source = match &cfg.weighting {
        Some(weighting) => {
            info!(
                endpoint = %weighting.endpoint,
                mode = ?weighting.mode,
                every = weighting.interval_secs,
                "weights will follow prometheus"
            );
            Some(PrometheusSource::new(weighting)?)
        }
        None => None,
    };

    let mut health_ticker = tokio::time::interval(Duration::from_secs(cfg.health_interval_secs));
    let mut weight_ticker = tokio::time::interval(Duration::from_secs(
        cfg.weighting
            .as_ref()
            .map(|weighting| weighting.interval_secs)
            .unwrap_or(3600),
    ));
    let timeout = Duration::from_millis(cfg.health_timeout_ms);

    let mut counters = Counters::default();

    loop {
        tokio::select! {
            _ = health_ticker.tick() => {
                probe_all(&mut slots, timeout).await;
                resolve_macs(&mut slots, &interface);

                counters.reconciles += 1;
                counters.rebuilds += sync(&mut plane, &mut slots, &drain)?;
                update_snapshot(&plane, &slots, &snapshot, &drain, &counters)?;
            }
            _ = weight_ticker.tick(), if weight_source.is_some() => {
                let source = weight_source.as_ref().expect("guarded by the select arm");
                match source.scores().await {
                    Ok(scores) => {
                        counters.weight_refreshes += 1;
                        if apply_weights(source, &mut slots, &scores) > 0 {
                            counters.rebuilds += sync(&mut plane, &mut slots, &drain)?;
                        }
                        update_snapshot(&plane, &slots, &snapshot, &drain, &counters)?;
                    }
                    Err(err) => {
                        counters.weight_failures += 1;
                        warn!(%err, "cannot refresh weights, keeping the current ones");
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("shutting down, detaching xdp program");
                break;
            }
        }
    }

    Ok(())
}

fn apply_weights(
    source: &PrometheusSource,
    slots: &mut [ServiceSlot],
    scores: &HashMap<String, f64>,
) -> usize {
    let mut changed = 0;

    for slot in slots.iter_mut() {
        let identities: Vec<String> = slot
            .backends
            .iter()
            .map(|backend| backend.metrics_identity.clone())
            .collect();
        let derived = source.weights_for(scores, &identities);

        for backend in slot.backends.iter_mut() {
            let Some(weight) = derived.get(&backend.metrics_identity) else {
                continue;
            };
            if backend.weight != *weight {
                info!(
                    service = %slot.name,
                    backend = %backend.key(),
                    from = backend.weight,
                    to = *weight,
                    "weight followed the metric"
                );
                backend.weight = *weight;
                changed += 1;
            }
        }
    }

    changed
}

fn build_slots(cfg: &Config) -> Vec<ServiceSlot> {
    let mut next_index = 0u32;
    let mut slots = Vec::with_capacity(cfg.services.len());

    for (svc_id, svc) in cfg.services.iter().enumerate() {
        let backends = svc
            .backends
            .iter()
            .map(|be| {
                let slot = BackendSlot {
                    index: next_index,
                    address: be.address,
                    port: be.port,
                    weight: be.weight,
                    metrics_identity: be.metrics_identity(),
                    mac_override: be.mac.as_deref().and_then(|m| config::parse_mac(m).ok()),
                    mac: None,
                    healthy: false,
                };
                next_index += 1;
                slot
            })
            .collect();

        slots.push(ServiceSlot {
            svc_id: svc_id as u32,
            name: svc.name.clone(),
            vip: svc.vip,
            port: svc.port,
            proto: svc.protocol,
            forwarding: svc.forwarding,
            dsr_source: svc.dsr_source,
            backends,
            table: vec![NO_BACKEND; MAGLEV_SIZE as usize],
        });
    }

    slots
}

fn publish_services(plane: &mut DataPlane, slots: &[ServiceSlot]) -> Result<()> {
    for slot in slots {
        plane.put_service(
            ServiceKey {
                vip: be32(slot.vip),
                port: be16(slot.port),
                proto: slot.proto.as_u8(),
                pad: 0,
            },
            ServiceInfo {
                svc_id: slot.svc_id,
                dsr_source: slot.dsr_source.map(be32).unwrap_or(0),
                mode: slot.forwarding.as_u8(),
                pad: [0; 3],
            },
        )?;
        info!(
            service = %slot.name,
            vip = %slot.vip,
            port = slot.port,
            protocol = ?slot.proto,
            forwarding = ?slot.forwarding,
            backends = slot.backends.len(),
            "service published to datapath"
        );
    }
    Ok(())
}

async fn probe_all(slots: &mut [ServiceSlot], timeout: Duration) {
    let mut probes = JoinSet::new();

    for (svc_pos, slot) in slots.iter().enumerate() {
        for (be_pos, backend) in slot.backends.iter().enumerate() {
            let address = backend.address;
            let port = backend.port;
            probes.spawn(async move {
                (
                    svc_pos,
                    be_pos,
                    health::probe_tcp(address, port, timeout).await,
                )
            });
        }
    }

    while let Some(done) = probes.join_next().await {
        let Ok((svc_pos, be_pos, healthy)) = done else {
            continue;
        };
        let backend = &mut slots[svc_pos].backends[be_pos];
        if backend.healthy != healthy {
            let state = if healthy { "up" } else { "down" };
            warn!(
                backend = %format!("{}:{}", backend.address, backend.port),
                "backend went {state}"
            );
        }
        backend.healthy = healthy;
    }
}

fn resolve_macs(slots: &mut [ServiceSlot], interface: &str) {
    for slot in slots.iter_mut() {
        for backend in slot.backends.iter_mut() {
            backend.mac = backend
                .mac_override
                .or_else(|| neigh::resolve(backend.address, interface));

            if backend.mac.is_none() && backend.healthy {
                warn!(
                    backend = %backend.address,
                    interface,
                    "no neighbour entry, backend cannot receive frames"
                );
            }
        }
    }
}

fn sync(plane: &mut DataPlane, slots: &mut [ServiceSlot], drain: &DrainList) -> Result<u64> {
    let mut rebuilds = 0u64;

    for slot in slots.iter_mut() {
        for backend in &slot.backends {
            let usable = backend.healthy && backend.mac.is_some();
            plane.put_backend(
                backend.index,
                Backend {
                    addr: be32(backend.address),
                    port: be16(backend.port),
                    flags: if usable { BACKEND_ACTIVE } else { 0 },
                    mac: backend.mac.unwrap_or_default(),
                    pad: [0; 2],
                },
            )?;
        }

        let weights: Vec<(u32, u32)> = slot
            .backends
            .iter()
            .filter(|b| b.healthy && b.mac.is_some() && !drain.contains(&b.key()))
            .map(|b| (b.index, b.weight))
            .collect();

        let lookup: std::collections::HashMap<u32, String> =
            slot.backends.iter().map(|b| (b.index, b.key())).collect();

        let candidates = maglev::expand_weighted(&weights, |idx| {
            lookup.get(&idx).cloned().unwrap_or_else(|| idx.to_string())
        });
        let table = maglev::build_table(&candidates, MAGLEV_SIZE as usize);

        if table != slot.table {
            plane.put_maglev_table(slot.svc_id, &table)?;
            slot.table = table;
            rebuilds += 1;
            info!(
                service = %slot.name,
                active = weights.len(),
                total = slot.backends.len(),
                "maglev table rebuilt"
            );
        }
    }

    Ok(rebuilds)
}

#[derive(Debug, Default)]
struct Counters {
    reconciles: u64,
    rebuilds: u64,
    weight_refreshes: u64,
    weight_failures: u64,
}

fn update_snapshot(
    plane: &DataPlane,
    slots: &[ServiceSlot],
    snapshot: &SharedSnapshot,
    drain: &DrainList,
    counters: &Counters,
) -> Result<()> {
    let global = plane
        .global_stats()?
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect();

    let mut backends = Vec::new();
    for slot in slots {
        for backend in &slot.backends {
            let key = backend.key();
            backends.push(BackendSample {
                service: slot.name.clone(),
                healthy: backend.healthy && backend.mac.is_some(),
                draining: drain.contains(&key),
                address: key,
                weight: backend.weight,
                stats: plane.backend_stats(backend.index)?,
            });
        }
    }

    if let Ok(mut guard) = snapshot.write() {
        guard.global = global;
        guard.backends = backends;
        guard.reconcile_count = counters.reconciles;
        guard.table_rebuild_count = counters.rebuilds;
        guard.weight_refresh_count = counters.weight_refreshes;
        guard.weight_failure_count = counters.weight_failures;
    }

    Ok(())
}

fn bump_memlock() -> Result<()> {
    let limit = libc::rlimit {
        rlim_cur: libc::RLIM_INFINITY,
        rlim_max: libc::RLIM_INFINITY,
    };
    let ret = unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK as _, &limit) };
    if ret != 0 {
        warn!("could not raise RLIMIT_MEMLOCK; map allocation may fail on kernels below 5.11");
    }
    Ok(())
}
