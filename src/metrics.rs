use std::{
    fmt::Write as _,
    sync::{Arc, RwLock},
};

use anyhow::{Context, Result};
use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Router};
use tokio::net::TcpListener;

use crate::types::StatVal;

#[derive(Debug, Default, Clone)]
pub struct BackendSample {
    pub service: String,
    pub address: String,
    pub healthy: bool,
    pub weight: u32,
    pub stats: StatVal,
}

#[derive(Debug, Default, Clone)]
pub struct Snapshot {
    pub global: Vec<(String, StatVal)>,
    pub backends: Vec<BackendSample>,
    pub reconcile_count: u64,
    pub table_rebuild_count: u64,
}

pub type SharedSnapshot = Arc<RwLock<Snapshot>>;

pub fn shared() -> SharedSnapshot {
    Arc::new(RwLock::new(Snapshot::default()))
}

pub async fn serve(addr: &str, snapshot: SharedSnapshot) -> Result<()> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("cannot bind metrics listener on {addr}"))?;

    let app = Router::new()
        .route("/metrics", get(render))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(snapshot);

    axum::serve(listener, app)
        .await
        .context("metrics server stopped")
}

async fn render(State(snapshot): State<SharedSnapshot>) -> impl IntoResponse {
    let Ok(snapshot) = snapshot.read() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, String::new());
    };

    let mut body = String::with_capacity(4096);

    let _ = writeln!(
        body,
        "# HELP xdplb_packets_total Packets counted by the datapath."
    );
    let _ = writeln!(body, "# TYPE xdplb_packets_total counter");
    for (name, value) in &snapshot.global {
        let _ = writeln!(
            body,
            "xdplb_packets_total{{verdict=\"{name}\"}} {}",
            value.packets
        );
    }

    let _ = writeln!(
        body,
        "# HELP xdplb_bytes_total Bytes counted by the datapath."
    );
    let _ = writeln!(body, "# TYPE xdplb_bytes_total counter");
    for (name, value) in &snapshot.global {
        let _ = writeln!(
            body,
            "xdplb_bytes_total{{verdict=\"{name}\"}} {}",
            value.bytes
        );
    }

    let _ = writeln!(
        body,
        "# HELP xdplb_backend_packets_total Packets forwarded per backend."
    );
    let _ = writeln!(body, "# TYPE xdplb_backend_packets_total counter");
    for sample in &snapshot.backends {
        let _ = writeln!(
            body,
            "xdplb_backend_packets_total{{service=\"{}\",backend=\"{}\"}} {}",
            sample.service, sample.address, sample.stats.packets
        );
    }

    let _ = writeln!(
        body,
        "# HELP xdplb_backend_up Backend health as observed by the control plane."
    );
    let _ = writeln!(body, "# TYPE xdplb_backend_up gauge");
    for sample in &snapshot.backends {
        let _ = writeln!(
            body,
            "xdplb_backend_up{{service=\"{}\",backend=\"{}\"}} {}",
            sample.service,
            sample.address,
            u8::from(sample.healthy)
        );
    }

    let _ = writeln!(
        body,
        "# HELP xdplb_backend_weight Weight used to build the maglev table."
    );
    let _ = writeln!(body, "# TYPE xdplb_backend_weight gauge");
    for sample in &snapshot.backends {
        let _ = writeln!(
            body,
            "xdplb_backend_weight{{service=\"{}\",backend=\"{}\"}} {}",
            sample.service, sample.address, sample.weight
        );
    }

    let _ = writeln!(
        body,
        "# HELP xdplb_reconcile_total Control plane reconcile loops completed."
    );
    let _ = writeln!(body, "# TYPE xdplb_reconcile_total counter");
    let _ = writeln!(body, "xdplb_reconcile_total {}", snapshot.reconcile_count);

    let _ = writeln!(
        body,
        "# HELP xdplb_table_rebuild_total Maglev tables written to the datapath."
    );
    let _ = writeln!(body, "# TYPE xdplb_table_rebuild_total counter");
    let _ = writeln!(
        body,
        "xdplb_table_rebuild_total {}",
        snapshot.table_rebuild_count
    );

    (StatusCode::OK, body)
}
