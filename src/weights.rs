use std::{collections::HashMap, time::Duration};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::config::{WeightMode, WeightingConfig};

pub struct PrometheusSource {
    client: reqwest::Client,
    url: String,
    query: String,
    instance_label: String,
    mode: WeightMode,
    min_weight: u32,
    max_weight: u32,
}

#[derive(Debug, Deserialize)]
struct QueryResponse {
    status: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    data: Option<QueryData>,
}

#[derive(Debug, Deserialize)]
struct QueryData {
    #[serde(default)]
    result: Vec<Series>,
}

#[derive(Debug, Deserialize)]
struct Series {
    #[serde(default)]
    metric: HashMap<String, String>,
    value: (f64, String),
}

impl PrometheusSource {
    pub fn new(cfg: &WeightingConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(cfg.timeout_ms))
            .build()
            .context("building the prometheus http client")?;

        Ok(Self {
            client,
            url: format!("{}/api/v1/query", cfg.endpoint.trim_end_matches('/')),
            query: cfg.query.clone(),
            instance_label: cfg.instance_label.clone(),
            mode: cfg.mode,
            min_weight: cfg.min_weight,
            max_weight: cfg.max_weight,
        })
    }

    pub async fn scores(&self) -> Result<HashMap<String, f64>> {
        let response = self
            .client
            .get(&self.url)
            .query(&[("query", self.query.as_str())])
            .send()
            .await
            .with_context(|| format!("querying {}", self.url))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("reading the prometheus response body")?;

        if !status.is_success() {
            bail!("prometheus answered {status}: {}", body.trim());
        }

        let parsed: QueryResponse =
            serde_json::from_str(&body).context("parsing the prometheus response")?;

        if parsed.status != "success" {
            bail!(
                "prometheus rejected the query: {}",
                parsed.error.unwrap_or_else(|| parsed.status.clone())
            );
        }

        let data = parsed.data.context("prometheus response carried no data")?;
        Ok(collect_scores(&data.result, &self.instance_label))
    }

    pub fn weights_for(
        &self,
        scores: &HashMap<String, f64>,
        targets: &[String],
    ) -> HashMap<String, u32> {
        derive_weights(scores, targets, self.mode, self.min_weight, self.max_weight)
    }
}

fn collect_scores(series: &[Series], instance_label: &str) -> HashMap<String, f64> {
    let mut scores = HashMap::new();

    for entry in series {
        let Some(label) = entry.metric.get(instance_label) else {
            continue;
        };
        let Ok(value) = entry.value.1.parse::<f64>() else {
            continue;
        };
        if !value.is_finite() {
            continue;
        }
        scores.insert(strip_port(label), value);
    }

    scores
}

fn strip_port(instance: &str) -> String {
    match instance.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host.to_string(),
        _ => instance.to_string(),
    }
}

pub fn derive_weights(
    scores: &HashMap<String, f64>,
    targets: &[String],
    mode: WeightMode,
    min_weight: u32,
    max_weight: u32,
) -> HashMap<String, u32> {
    let mut raw: Vec<(&String, f64)> = Vec::with_capacity(targets.len());

    for target in targets {
        let Some(value) = scores.get(target) else {
            continue;
        };
        let score = match mode {
            WeightMode::Proportional => *value,
            WeightMode::Inverse => {
                if *value <= 0.0 {
                    continue;
                }
                1.0 / *value
            }
        };
        if !score.is_finite() || score <= 0.0 {
            continue;
        }
        raw.push((target, score));
    }

    let peak = raw.iter().map(|(_, score)| *score).fold(0.0f64, f64::max);
    if peak <= 0.0 {
        return HashMap::new();
    }

    raw.into_iter()
        .map(|(target, score)| {
            let scaled = (max_weight as f64 * score / peak).round() as i64;
            let clamped = scaled.clamp(min_weight as i64, max_weight as i64) as u32;
            (target.clone(), clamped)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scores(entries: &[(&str, f64)]) -> HashMap<String, f64> {
        entries
            .iter()
            .map(|(key, value)| (key.to_string(), *value))
            .collect()
    }

    fn targets(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|entry| entry.to_string()).collect()
    }

    #[test]
    fn proportional_mode_scales_against_the_strongest_backend() {
        let derived = derive_weights(
            &scores(&[("10.0.0.11", 100.0), ("10.0.0.12", 50.0)]),
            &targets(&["10.0.0.11", "10.0.0.12"]),
            WeightMode::Proportional,
            1,
            16,
        );

        assert_eq!(derived["10.0.0.11"], 16);
        assert_eq!(derived["10.0.0.12"], 8);
    }

    #[test]
    fn inverse_mode_penalises_the_slower_backend() {
        let derived = derive_weights(
            &scores(&[("10.0.0.11", 10.0), ("10.0.0.12", 20.0)]),
            &targets(&["10.0.0.11", "10.0.0.12"]),
            WeightMode::Inverse,
            1,
            16,
        );

        assert_eq!(derived["10.0.0.11"], 16);
        assert_eq!(derived["10.0.0.12"], 8);
    }

    #[test]
    fn a_backend_without_a_sample_keeps_its_previous_weight() {
        let derived = derive_weights(
            &scores(&[("10.0.0.11", 100.0)]),
            &targets(&["10.0.0.11", "10.0.0.12"]),
            WeightMode::Proportional,
            1,
            16,
        );

        assert!(derived.contains_key("10.0.0.11"));
        assert!(
            !derived.contains_key("10.0.0.12"),
            "a missing sample must not be read as zero capacity"
        );
    }

    #[test]
    fn a_nearly_idle_backend_still_gets_the_floor() {
        let derived = derive_weights(
            &scores(&[("10.0.0.11", 1000.0), ("10.0.0.12", 0.001)]),
            &targets(&["10.0.0.11", "10.0.0.12"]),
            WeightMode::Proportional,
            2,
            16,
        );

        assert_eq!(derived["10.0.0.12"], 2, "min_weight must be respected");
    }

    #[test]
    fn non_positive_and_infinite_values_are_ignored() {
        let derived = derive_weights(
            &scores(&[
                ("10.0.0.11", 100.0),
                ("10.0.0.12", 0.0),
                ("10.0.0.13", -5.0),
                ("10.0.0.14", f64::INFINITY),
            ]),
            &targets(&["10.0.0.11", "10.0.0.12", "10.0.0.13", "10.0.0.14"]),
            WeightMode::Proportional,
            1,
            16,
        );

        assert_eq!(derived.len(), 1);
        assert!(derived.contains_key("10.0.0.11"));
    }

    #[test]
    fn no_usable_sample_yields_no_change() {
        let derived = derive_weights(
            &scores(&[("10.0.0.11", 0.0)]),
            &targets(&["10.0.0.11"]),
            WeightMode::Proportional,
            1,
            16,
        );

        assert!(derived.is_empty());
    }

    #[test]
    fn instance_labels_lose_their_exporter_port() {
        assert_eq!(strip_port("10.0.0.11:9100"), "10.0.0.11");
        assert_eq!(strip_port("10.0.0.11"), "10.0.0.11");
        assert_eq!(strip_port("backend-a.internal:9100"), "backend-a.internal");
        assert_eq!(strip_port("backend-a.internal"), "backend-a.internal");
    }

    #[test]
    fn scores_are_read_from_a_prometheus_vector() {
        let body = r#"{
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": [
                    {"metric": {"instance": "10.0.0.11:9100"}, "value": [1699999999.0, "42.5"]},
                    {"metric": {"instance": "10.0.0.12:9100"}, "value": [1699999999.0, "13"]},
                    {"metric": {"job": "node"}, "value": [1699999999.0, "99"]},
                    {"metric": {"instance": "10.0.0.13:9100"}, "value": [1699999999.0, "NaN"]}
                ]
            }
        }"#;

        let parsed: QueryResponse = serde_json::from_str(body).expect("body must parse");
        let collected = collect_scores(&parsed.data.unwrap().result, "instance");

        assert_eq!(collected["10.0.0.11"], 42.5);
        assert_eq!(collected["10.0.0.12"], 13.0);
        assert_eq!(
            collected.len(),
            2,
            "series without the label or with NaN are skipped"
        );
    }

    #[test]
    fn a_failed_query_is_reported_not_swallowed() {
        let body = r#"{"status":"error","errorType":"bad_data","error":"parse error"}"#;
        let parsed: QueryResponse = serde_json::from_str(body).expect("body must parse");

        assert_eq!(parsed.status, "error");
        assert_eq!(parsed.error.as_deref(), Some("parse error"));
    }
}
