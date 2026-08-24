use std::{
    net::Ipv4Addr,
    os::fd::{AsFd, OwnedFd},
};

use anyhow::{Context, Result};
use aya::{programs::Xdp, Ebpf};
use xdp_lb::{
    dataplane::DataPlane,
    packet::{arp_frame, tcp_syn, Endpoint},
    progtest,
    types::{
        be16, be32, Backend, RateConfig, ServiceInfo, ServiceKey, BACKEND_ACTIVE, MAGLEV_SIZE,
        MODE_DSR, MODE_NAT, NO_BACKEND,
    },
};

const REPEAT_IN_KERNEL: u32 = 1_000_000;
const SAMPLES: u32 = 20_000;

const CLIENT: Endpoint = Endpoint {
    mac: [0x02, 0, 0, 0, 0, 0x10],
    address: Ipv4Addr::new(10, 1, 0, 10),
    port: 40000,
};

const VIP: Endpoint = Endpoint {
    mac: [0x02, 0, 0, 0, 0, 0x01],
    address: Ipv4Addr::new(10, 0, 0, 100),
    port: 80,
};

const BACKEND_MAC: [u8; 6] = [0x02, 0, 0, 0, 0, 0x11];
const BACKEND_ADDR: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 11);
const DSR_SOURCE: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 1);

struct Bench {
    _ebpf: Ebpf,
    program: OwnedFd,
    plane: DataPlane,
}

impl Bench {
    fn load() -> Result<Self> {
        let mut ebpf = Ebpf::load(xdp_lb::object::bytes()).context("loading the object")?;

        let program: &mut Xdp = ebpf
            .program_mut("xdp_lb")
            .context("no program named xdp_lb")?
            .try_into()?;
        program.load().context("verifier rejected the program")?;

        let fd = program.fd()?.as_fd().try_clone_to_owned()?;
        let plane = DataPlane::attach_maps(&mut ebpf)?;

        Ok(Self {
            _ebpf: ebpf,
            program: fd,
            plane,
        })
    }

    fn configure(&mut self, mode: u8) -> Result<()> {
        self.plane.put_service(
            ServiceKey {
                vip: be32(VIP.address),
                port: be16(VIP.port),
                proto: 6,
                pad: 0,
            },
            ServiceInfo {
                svc_id: 0,
                dsr_source: be32(DSR_SOURCE),
                mode,
                pad: [0; 3],
            },
        )?;
        self.plane.put_backend(
            0,
            Backend {
                addr: be32(BACKEND_ADDR),
                port: be16(if mode == MODE_DSR { VIP.port } else { 8080 }),
                flags: BACKEND_ACTIVE,
                mac: BACKEND_MAC,
                pad: [0; 2],
            },
        )?;
        self.plane
            .put_maglev_table(0, &vec![0u32; MAGLEV_SIZE as usize])?;
        self.plane.put_rate_config(RateConfig::disabled())?;
        Ok(())
    }

    fn empty_table(&mut self) -> Result<()> {
        self.plane
            .put_maglev_table(0, &vec![NO_BACKEND; MAGLEV_SIZE as usize])?;
        Ok(())
    }

    fn rate_limit(&mut self, config: RateConfig) -> Result<()> {
        self.plane.put_rate_config(config)
    }

    fn stat(&self, name: &str) -> Result<u64> {
        Ok(self
            .plane
            .global_stats()?
            .into_iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, value)| value.packets)
            .unwrap_or(0))
    }

    fn once(&self, packet: &[u8]) -> Result<progtest::Outcome> {
        progtest::run(self.program.as_fd(), packet, 1).context("BPF_PROG_TEST_RUN failed")
    }

    fn in_kernel_loop(&self, packet: &[u8]) -> Result<f64> {
        let outcome = progtest::run(self.program.as_fd(), packet, REPEAT_IN_KERNEL)?;
        Ok(outcome.duration_ns as f64)
    }

    fn per_call(&self, mut build: impl FnMut(u32) -> Vec<u8>) -> Result<(u32, f64)> {
        let mut total = 0f64;
        let mut verdict = 0;

        for index in 0..SAMPLES {
            let packet = build(index);
            let outcome = self.once(&packet)?;
            total += outcome.duration_ns as f64;
            verdict = outcome.verdict;
        }

        Ok((verdict, total / SAMPLES as f64))
    }
}

fn verdict_name(verdict: u32) -> &'static str {
    match verdict {
        progtest::XDP_ABORTED => "ABORTED",
        progtest::XDP_DROP => "DROP",
        progtest::XDP_PASS => "PASS",
        progtest::XDP_TX => "TX",
        progtest::XDP_REDIRECT => "REDIRECT",
        _ => "?",
    }
}

fn row(scenario: &str, verdict: u32, raw: f64, overhead: f64) {
    let corrected = (raw - overhead).max(0.0);
    let mpps = if corrected > 0.0 {
        1_000.0 / corrected
    } else {
        f64::INFINITY
    };
    println!(
        "{scenario:<30} {:>8} {:>9.0} {:>10.0} {:>13.1}",
        verdict_name(verdict),
        raw,
        corrected,
        mpps
    );
}

fn scenario(
    name: &str,
    overhead: f64,
    measure: impl FnOnce(&mut Bench) -> Result<(u32, f64)>,
) -> Result<()> {
    let mut bench = Bench::load()?;
    let (verdict, raw) = measure(&mut bench)?;
    row(name, verdict, raw, overhead);
    Ok(())
}

fn varying_port(base: u16) -> impl FnMut(u32) -> Vec<u8> {
    move |index| {
        tcp_syn(
            Endpoint {
                port: base.wrapping_add((index % SAMPLES as u16 as u32) as u16),
                ..CLIENT
            },
            VIP,
        )
    }
}

fn describe_environment() {
    println!("arch    {}", std::env::consts::ARCH);
    if let Ok(release) = std::fs::read_to_string("/proc/sys/kernel/osrelease") {
        println!("kernel  {}", release.trim());
    }
    if let Ok(info) = std::fs::read_to_string("/proc/cpuinfo") {
        if let Some(model) = info
            .lines()
            .find(|line| line.starts_with("model name") || line.starts_with("Model"))
            .and_then(|line| line.split(':').nth(1))
        {
            println!("cpu     {}", model.trim());
        }
    }
    println!("cpus    {}", aya::util::nr_cpus().unwrap_or(0));
}

fn main() -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        anyhow::bail!("the benchmark loads a BPF program and needs root");
    }

    println!("xdp-lb datapath microbenchmark\n");
    describe_environment();

    let forward = tcp_syn(CLIENT, VIP);
    let arp = arp_frame(CLIENT.mac, VIP.mac);

    let (miss, hit, pass) = {
        let mut probe = Bench::load()?;
        probe.configure(MODE_NAT)?;
        progtest::run(probe.program.as_fd(), &forward, 1000)?;
        (
            probe.stat("conntrack_miss")?,
            probe.stat("conntrack_hit")?,
            probe.stat("pass")?,
        )
    };

    println!("\nmethod");
    println!("  BPF_PROG_TEST_RUN does not restore the packet between its own repeats.");
    println!(
        "  1000 in-kernel repeats of one nat flow produced miss={miss} hit={hit} pass={pass}:"
    );
    println!("  only the first repeat took the intended path, the rest saw the already");
    println!("  rewritten packet and left early. So any path that edits the packet must be");
    println!("  measured one call at a time, each call handed a pristine copy.");
    println!();
    println!("  one call at a time costs extra: the kernel times a single invocation instead");
    println!("  of amortising the clock over a million. That overhead is calibrated on a path");
    println!("  that does not touch the packet, where both methods are valid, and subtracted.");

    let (in_kernel, per_call) = {
        let calibrator = Bench::load()?;
        let in_kernel = calibrator.in_kernel_loop(&arp)?;
        let (_, per_call) = calibrator.per_call(|_| arp.clone())?;
        (in_kernel, per_call)
    };
    let overhead = (per_call - in_kernel).max(0.0);

    println!();
    println!("  non-ip frame, {REPEAT_IN_KERNEL} in-kernel repeats : {in_kernel:.0} ns/packet");
    println!("  non-ip frame, {SAMPLES} single calls        : {per_call:.0} ns/packet");
    println!("  per-call overhead subtracted below           : {overhead:.0} ns");

    println!(
        "\n{:<30} {:>8} {:>9} {:>10} {:>13}",
        "scenario", "verdict", "raw ns", "net ns", "Mpkt/s/core"
    );
    println!("{}", "-".repeat(75));

    let stray = tcp_syn(
        CLIENT,
        Endpoint {
            address: Ipv4Addr::new(10, 0, 0, 99),
            ..VIP
        },
    );

    scenario("non-ip frame, passed", overhead, |bench| {
        bench.per_call(|_| arp.clone())
    })?;

    scenario("unknown destination, passed", overhead, |bench| {
        bench.per_call(|_| stray.clone())
    })?;

    scenario("nat, established flow", overhead, |bench| {
        bench.configure(MODE_NAT)?;
        bench.once(&forward)?;
        bench.per_call(|_| forward.clone())
    })?;

    scenario("nat, new flow", overhead, |bench| {
        bench.configure(MODE_NAT)?;
        bench.per_call(varying_port(1024))
    })?;

    scenario("dsr, established flow", overhead, |bench| {
        bench.configure(MODE_DSR)?;
        bench.once(&forward)?;
        bench.per_call(|_| forward.clone())
    })?;

    scenario("dsr, new flow", overhead, |bench| {
        bench.configure(MODE_DSR)?;
        bench.per_call(varying_port(1024))
    })?;

    scenario("no backend, dropped", overhead, |bench| {
        bench.configure(MODE_NAT)?;
        bench.empty_table()?;
        bench.per_call(varying_port(1024))
    })?;

    scenario("rate limited, dropped", overhead, |bench| {
        bench.configure(MODE_NAT)?;
        bench.rate_limit(RateConfig::per_cpu(1, 1))?;
        bench.once(&tcp_syn(
            Endpoint {
                port: 9999,
                ..CLIENT
            },
            VIP,
        ))?;
        bench.per_call(varying_port(20000))
    })?;

    println!("\nMpkt/s/core is one second divided by net ns: the ceiling this datapath puts");
    println!("on a single core. It is not a throughput measurement of any real system —");
    println!("no NIC, no driver, no contention. Treat it as an upper bound.");

    Ok(())
}
