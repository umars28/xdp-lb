use std::{
    fmt::Write as _,
    sync::{Arc, RwLock},
};

use anyhow::{Context, Result};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use tokio::net::TcpListener;

use crate::{drain::DrainList, types::StatVal};

#[derive(Debug, Default, Clone)]
pub struct BackendSample {
    pub service: String,
    pub address: String,
    pub healthy: bool,
    pub draining: bool,
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

#[derive(Clone)]
pub struct AppState {
    pub snapshot: SharedSnapshot,
    pub drain: DrainList,
}

#[derive(Debug, Deserialize)]
pub struct BackendQuery {
    backend: String,
}

pub async fn serve(addr: &str, state: AppState) -> Result<()> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("cannot bind metrics listener on {addr}"))?;

    let app = Router::new()
        .route("/metrics", get(render))
        .route("/healthz", get(|| async { "ok" }))
        .route("/drain", get(list_drained).post(start_drain))
        .route("/undrain", post(stop_drain))
        .with_state(state);

    axum::serve(listener, app)
        .await
        .context("metrics server stopped")
}

async fn list_drained(State(state): State<AppState>) -> impl IntoResponse {
    let mut body = String::new();
    for entry in state.drain.entries() {
        let _ = writeln!(body, "{entry}");
    }
    (StatusCode::OK, body)
}

async fn start_drain(
    State(state): State<AppState>,
    Query(query): Query<BackendQuery>,
) -> impl IntoResponse {
    if !known_backend(&state, &query.backend) {
        return (
            StatusCode::NOT_FOUND,
            format!("no backend named {}\n", query.backend),
        );
    }
    let changed = state.drain.drain(&query.backend);
    let verb = if changed {
        "draining"
    } else {
        "already draining"
    };
    (StatusCode::OK, format!("{} {verb}\n", query.backend))
}

async fn stop_drain(
    State(state): State<AppState>,
    Query(query): Query<BackendQuery>,
) -> impl IntoResponse {
    if !known_backend(&state, &query.backend) {
        return (
            StatusCode::NOT_FOUND,
            format!("no backend named {}\n", query.backend),
        );
    }
    let changed = state.drain.undrain(&query.backend);
    let verb = if changed {
        "serving"
    } else {
        "already serving"
    };
    (StatusCode::OK, format!("{} {verb}\n", query.backend))
}

fn known_backend(state: &AppState, backend: &str) -> bool {
    match state.snapshot.read() {
        Ok(snapshot) => snapshot
            .backends
            .iter()
            .any(|sample| sample.address == backend),
        Err(_) => false,
    }
}

async fn render(State(state): State<AppState>) -> impl IntoResponse {
    let Ok(snapshot) = state.snapshot.read() else {
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
        "# HELP xdplb_backend_draining Backend excluded from new flows but still serving established ones."
    );
    let _ = writeln!(body, "# TYPE xdplb_backend_draining gauge");
    for sample in &snapshot.backends {
        let _ = writeln!(
            body,
            "xdplb_backend_draining{{service=\"{}\",backend=\"{}\"}} {}",
            sample.service,
            sample.address,
            u8::from(sample.draining)
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
