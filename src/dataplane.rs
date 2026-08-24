use anyhow::{Context, Result};
use aya::{
    maps::{Array, HashMap as BpfHashMap, MapData, PerCpuArray},
    Ebpf,
};

use crate::types::{
    Backend, RateConfig, ServiceInfo, ServiceKey, StatVal, MAGLEV_SIZE, MAX_SERVICES, STAT_NAMES,
};

pub struct DataPlane {
    services: BpfHashMap<MapData, ServiceKey, ServiceInfo>,
    backends: Array<MapData, Backend>,
    maglev: Array<MapData, u32>,
    rate_config: Array<MapData, RateConfig>,
    stats: PerCpuArray<MapData, StatVal>,
    backend_stats: PerCpuArray<MapData, StatVal>,
}

impl DataPlane {
    pub fn attach_maps(ebpf: &mut Ebpf) -> Result<Self> {
        Ok(Self {
            services: BpfHashMap::try_from(take(ebpf, "services")?)?,
            backends: Array::try_from(take(ebpf, "backends")?)?,
            maglev: Array::try_from(take(ebpf, "maglev")?)?,
            rate_config: Array::try_from(take(ebpf, "rate_config")?)?,
            stats: PerCpuArray::try_from(take(ebpf, "stats")?)?,
            backend_stats: PerCpuArray::try_from(take(ebpf, "backend_stats")?)?,
        })
    }

    pub fn put_rate_config(&mut self, config: RateConfig) -> Result<()> {
        self.rate_config
            .set(0, config, 0)
            .context("writing rate_config map")
    }

    pub fn put_service(&mut self, key: ServiceKey, info: ServiceInfo) -> Result<()> {
        self.services
            .insert(key, info, 0)
            .context("writing services map")
    }

    pub fn put_backend(&mut self, index: u32, backend: Backend) -> Result<()> {
        self.backends
            .set(index, backend, 0)
            .context("writing backends map")
    }

    pub fn put_maglev_table(&mut self, svc_id: u32, table: &[u32]) -> Result<()> {
        anyhow::ensure!(
            svc_id < MAX_SERVICES,
            "service id {svc_id} exceeds datapath capacity"
        );
        anyhow::ensure!(
            table.len() == MAGLEV_SIZE as usize,
            "maglev table has {} slots, datapath expects {MAGLEV_SIZE}",
            table.len()
        );

        let base = svc_id * MAGLEV_SIZE;
        for (offset, chosen) in table.iter().enumerate() {
            self.maglev
                .set(base + offset as u32, *chosen, 0)
                .context("writing maglev map")?;
        }
        Ok(())
    }

    pub fn global_stats(&self) -> Result<Vec<(&'static str, StatVal)>> {
        let mut out = Vec::with_capacity(STAT_NAMES.len());
        for (index, name) in STAT_NAMES.iter().enumerate() {
            out.push((*name, self.sum_percpu(&self.stats, index as u32)?));
        }
        Ok(out)
    }

    pub fn backend_stats(&self, index: u32) -> Result<StatVal> {
        self.sum_percpu(&self.backend_stats, index)
    }

    fn sum_percpu(&self, map: &PerCpuArray<MapData, StatVal>, index: u32) -> Result<StatVal> {
        let per_cpu = map
            .get(&index, 0)
            .with_context(|| format!("reading per-cpu stat {index}"))?;

        let mut total = StatVal::default();
        for value in per_cpu.iter() {
            total.packets += value.packets;
            total.bytes += value.bytes;
        }
        Ok(total)
    }
}

fn take(ebpf: &mut Ebpf, name: &str) -> Result<aya::maps::Map> {
    ebpf.take_map(name)
        .with_context(|| format!("BPF object has no map named {name:?}"))
}
