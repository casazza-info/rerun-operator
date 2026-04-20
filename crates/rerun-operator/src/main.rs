//! rerun-operator — Kubernetes controller for RerunDashboard CRs.
//!
//! See `rerun-operator-api` for the CRD types.

mod blueprint;
mod reconciler;
mod resources;

use std::sync::Arc;

use futures::StreamExt;
use kube::runtime::{Controller, watcher::Config};
use kube::{Api, Client, CustomResourceExt};
use tracing::{error, info};

use hephaestus_operator_lib::{crd as op_crd, telemetry};
use rerun_operator_api::v1alpha1::RerunDashboard;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    telemetry::init_tracing("info,rerun_operator=debug");

    if std::env::args().nth(1).as_deref() == Some("export-crds") {
        op_crd::print_json_stream(&[RerunDashboard::crd()]);
        return Ok(());
    }

    info!("starting rerun-operator");

    let registry = prometheus::Registry::new();
    let metrics_port: u16 = std::env::var("METRICS_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let metrics_registry = registry.clone();
    tokio::spawn(async move { telemetry::serve_metrics(metrics_port, metrics_registry).await });

    let client = Client::try_default().await?;
    let ctx = Arc::new(reconciler::Context {
        client: client.clone(),
    });

    let dashes: Api<RerunDashboard> = Api::all(client.clone());
    let ctrl = Controller::new(dashes, Config::default())
        .shutdown_on_signal()
        .run(reconciler::reconcile, reconciler::error_policy, ctx)
        .for_each(|res| async move {
            match res {
                Ok(o) => info!(?o, "reconciled RerunDashboard"),
                Err(e) => error!(%e, "RerunDashboard reconcile error"),
            }
        });

    info!("controller started, watching RerunDashboard resources");
    ctrl.await;
    Ok(())
}
