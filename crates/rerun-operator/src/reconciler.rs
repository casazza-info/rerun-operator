//! Reconcile a RerunDashboard into ConfigMap + Deployment + Service + PVC.

use std::sync::Arc;
use std::time::Duration;

use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{ConfigMap, PersistentVolumeClaim, Service};
use kube::api::{Api, Patch, PatchParams};
use kube::runtime::controller::Action;
use kube::{Resource, ResourceExt};
use serde_json::json;
use tracing::{info, warn};

use rerun_operator_api::v1alpha1::{
    DashboardPhase, Endpoints, RerunDashboard, RerunDashboardStatus, StorageMode, Visibility,
};

use crate::resources::{self, MANAGER};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("kube error: {0}")]
    Kube(#[from] kube::Error),

    #[error("missing namespace on RerunDashboard {0}")]
    MissingNamespace(String),
}

pub struct Context {
    pub client: kube::Client,
}

pub async fn reconcile(dash: Arc<RerunDashboard>, ctx: Arc<Context>) -> Result<Action, Error> {
    let name = dash.name_any();
    let ns = dash
        .metadata
        .namespace
        .clone()
        .ok_or_else(|| Error::MissingNamespace(name.clone()))?;
    info!(%name, %ns, "reconcile RerunDashboard");

    apply_configmap(&ctx, &ns, &dash).await?;
    if dash.spec.storage.mode == StorageMode::Persistent
        && dash.spec.storage.existing_claim.is_none()
    {
        apply_pvc(&ctx, &ns, &dash).await?;
    }
    apply_deployment(&ctx, &ns, &dash).await?;
    apply_service(&ctx, &ns, &dash).await?;

    patch_status(&ctx, &ns, &dash).await?;

    // Requeue to refresh status periodically.
    Ok(Action::requeue(Duration::from_secs(60)))
}

pub fn error_policy(dash: Arc<RerunDashboard>, err: &Error, _ctx: Arc<Context>) -> Action {
    warn!(name = %dash.name_any(), %err, "reconcile error");
    Action::requeue(Duration::from_secs(30))
}

async fn apply_configmap(ctx: &Context, ns: &str, dash: &RerunDashboard) -> Result<(), Error> {
    let cm = resources::build_configmap(dash);
    let api: Api<ConfigMap> = Api::namespaced(ctx.client.clone(), ns);
    let name = cm.name_any();
    api.patch(&name, &PatchParams::apply(MANAGER).force(), &Patch::Apply(&cm))
        .await?;
    Ok(())
}

async fn apply_deployment(ctx: &Context, ns: &str, dash: &RerunDashboard) -> Result<(), Error> {
    let dep = resources::build_deployment(dash);
    let api: Api<Deployment> = Api::namespaced(ctx.client.clone(), ns);
    let name = dep.name_any();
    api.patch(&name, &PatchParams::apply(MANAGER).force(), &Patch::Apply(&dep))
        .await?;
    Ok(())
}

async fn apply_service(ctx: &Context, ns: &str, dash: &RerunDashboard) -> Result<(), Error> {
    let svc = resources::build_service(dash);
    let api: Api<Service> = Api::namespaced(ctx.client.clone(), ns);
    let name = svc.name_any();
    api.patch(&name, &PatchParams::apply(MANAGER).force(), &Patch::Apply(&svc))
        .await?;
    Ok(())
}

async fn apply_pvc(ctx: &Context, ns: &str, dash: &RerunDashboard) -> Result<(), Error> {
    let Some(pvc) = resources::build_pvc(dash) else {
        return Ok(());
    };
    let api: Api<PersistentVolumeClaim> = Api::namespaced(ctx.client.clone(), ns);
    let name = pvc.name_any();
    api.patch(&name, &PatchParams::apply(MANAGER).force(), &Patch::Apply(&pvc))
        .await?;
    Ok(())
}

async fn patch_status(ctx: &Context, ns: &str, dash: &RerunDashboard) -> Result<(), Error> {
    let name = dash.name_any();
    let dep_api: Api<Deployment> = Api::namespaced(ctx.client.clone(), ns);
    let dep = dep_api.get_opt(&name).await?;
    let ready_replicas = dep
        .as_ref()
        .and_then(|d| d.status.as_ref())
        .and_then(|s| s.ready_replicas)
        .unwrap_or(0);

    let ingest = &dash.spec.ingest;
    let host = resources::service_dns(dash);
    let web = format!(
        "http://{host}:{web}/?url=ws://{host}:{ws}",
        web = ingest.web_port,
        ws = ingest.port
    );
    let ingest_url = match ingest.protocol {
        rerun_operator_api::v1alpha1::IngestProtocol::Tcp => {
            format!("tcp://{host}:{port}", port = ingest.port)
        }
        rerun_operator_api::v1alpha1::IngestProtocol::Grpc => {
            format!("grpc://{host}:{port}", port = ingest.port)
        }
    };
    let public_url = match dash.spec.ingress.visibility {
        Visibility::Public => dash
            .spec
            .ingress
            .public_hostname
            .as_ref()
            .map(|h| format!("https://{h}")),
        Visibility::Cluster => None,
    };

    let phase = if ready_replicas >= 1 {
        DashboardPhase::Ready
    } else {
        DashboardPhase::Provisioning
    };

    let status = RerunDashboardStatus {
        phase,
        endpoints: Some(Endpoints {
            web,
            ingest: ingest_url,
            public_url,
        }),
        connected_loggers: 0,
        last_activity_time: None,
        persisted_bytes: None,
        error_message: None,
        last_transition_time: Some(chrono::Utc::now().to_rfc3339()),
        conditions: vec![],
    };

    let dash_api: Api<RerunDashboard> = Api::namespaced(ctx.client.clone(), ns);
    let patch = json!({
        "apiVersion": RerunDashboard::api_version(&()),
        "kind": RerunDashboard::kind(&()),
        "status": status,
    });
    dash_api
        .patch_status(&name, &PatchParams::apply(MANAGER), &Patch::Merge(&patch))
        .await?;
    Ok(())
}
