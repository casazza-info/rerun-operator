//! Build Kubernetes objects owned by a RerunDashboard.

use std::collections::BTreeMap;

use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec};
use k8s_openapi::api::core::v1::{
    ConfigMap, ConfigMapVolumeSource, Container, ContainerPort, PersistentVolumeClaimVolumeSource,
    PodSpec, PodTemplateSpec, Service, ServicePort, ServiceSpec, Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, OwnerReference};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::ResourceExt;
use rerun_operator_api::v1alpha1::{DataSourceKind, RerunDashboard};

use crate::blueprint::{self, DEFAULT_REPLAY_MOUNT, RenderedLaunch};

pub const MANAGER: &str = "rerun-operator";
const BLUEPRINT_KEY: &str = "blueprint.py";
const BLUEPRINT_MOUNT: &str = "/opt/rerun/blueprint";

/// Default viewer image: vanilla Python; the container `pip install`s
/// rerun-sdk on boot. A purpose-built image is left to a future release.
const VIEWER_IMAGE: &str = "python:3.13-slim";

pub fn labels(dashboard_name: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("app.kubernetes.io/name".into(), "rerun-dashboard".into()),
        ("app.kubernetes.io/instance".into(), dashboard_name.into()),
        ("app.kubernetes.io/managed-by".into(), MANAGER.into()),
        ("rerun.nixlab.io/dashboard".into(), dashboard_name.into()),
    ])
}

fn owner_ref(dash: &RerunDashboard) -> OwnerReference {
    OwnerReference {
        api_version: "rerun.nixlab.io/v1alpha1".into(),
        kind: "RerunDashboard".into(),
        name: dash.name_any(),
        uid: dash.uid().unwrap_or_default(),
        controller: Some(true),
        block_owner_deletion: Some(true),
    }
}

fn render_launch(dash: &RerunDashboard) -> RenderedLaunch {
    let name = dash.name_any();
    let app_id = dash
        .spec
        .application_id
        .clone()
        .unwrap_or_else(|| name.clone());
    blueprint::render(&app_id, &dash.spec)
}

/// Build the blueprint ConfigMap. Returns `None` for FileReplay (no Python
/// module needed).
pub fn build_configmap(dash: &RerunDashboard) -> Option<ConfigMap> {
    let name = dash.name_any();
    let rendered = render_launch(dash);
    let python = rendered.python?;

    Some(ConfigMap {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(format!("{name}-blueprint")),
            namespace: dash.metadata.namespace.clone(),
            labels: Some(labels(&name)),
            owner_references: Some(vec![owner_ref(dash)]),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(BLUEPRINT_KEY.into(), python)])),
        ..Default::default()
    })
}

/// The viewer Deployment name. The CR name is reserved for the
/// (dashboard-named) `Service`, which is what loggers connect to, and must
/// not collide with unrelated Deployments a user may already run in the same
/// namespace (e.g. a training pod sharing the CR name).
fn deployment_name(dash: &RerunDashboard) -> String {
    format!("{}-viewer", dash.name_any())
}

pub fn build_deployment(dash: &RerunDashboard) -> Deployment {
    let name = dash.name_any();
    let dep_name = deployment_name(dash);
    let lbls = labels(&name);
    let rendered = render_launch(dash);

    let mut volumes: Vec<Volume> = Vec::new();
    let mut mounts: Vec<VolumeMount> = Vec::new();

    if rendered.python.is_some() {
        volumes.push(Volume {
            name: "blueprint".into(),
            config_map: Some(ConfigMapVolumeSource {
                name: format!("{name}-blueprint"),
                ..Default::default()
            }),
            ..Default::default()
        });
        mounts.push(VolumeMount {
            name: "blueprint".into(),
            mount_path: BLUEPRINT_MOUNT.into(),
            read_only: Some(true),
            ..Default::default()
        });
    }

    // FileReplay (or Mixed with explicit files): mount the user-provided PVC.
    if matches!(
        dash.spec.data_source.kind,
        DataSourceKind::FileReplay | DataSourceKind::Mixed
    ) && let Some(files) = &dash.spec.data_source.files
    {
        volumes.push(Volume {
            name: "recordings".into(),
            persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
                claim_name: files.pvc.clone(),
                read_only: Some(false),
            }),
            ..Default::default()
        });
        mounts.push(VolumeMount {
            name: "recordings".into(),
            mount_path: files
                .mount_path
                .clone()
                .unwrap_or_else(|| DEFAULT_REPLAY_MOUNT.to_string()),
            ..Default::default()
        });
    }

    let mut ports: Vec<ContainerPort> = Vec::new();
    if let Some(wp) = rendered.web_port {
        ports.push(ContainerPort {
            name: Some("web".into()),
            container_port: wp as i32,
            protocol: Some("TCP".into()),
            ..Default::default()
        });
    }
    if let Some(lp) = rendered.live_port {
        ports.push(ContainerPort {
            name: Some("ingest".into()),
            container_port: lp as i32,
            protocol: Some("TCP".into()),
            ..Default::default()
        });
    }

    let container = Container {
        name: "viewer".into(),
        image: Some(VIEWER_IMAGE.into()),
        command: Some(vec!["sh".into(), "-c".into(), rendered.shell]),
        ports: if ports.is_empty() { None } else { Some(ports) },
        volume_mounts: if mounts.is_empty() { None } else { Some(mounts) },
        resources: dash.spec.resources.clone(),
        ..Default::default()
    };

    Deployment {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(dep_name),
            namespace: dash.metadata.namespace.clone(),
            labels: Some(lbls.clone()),
            owner_references: Some(vec![owner_ref(dash)]),
            ..Default::default()
        },
        spec: Some(DeploymentSpec {
            replicas: Some(1),
            selector: LabelSelector {
                match_labels: Some(lbls.clone()),
                ..Default::default()
            },
            strategy: Some(k8s_openapi::api::apps::v1::DeploymentStrategy {
                type_: Some("Recreate".into()),
                ..Default::default()
            }),
            template: PodTemplateSpec {
                metadata: Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                    labels: Some(lbls),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    containers: vec![container],
                    volumes: if volumes.is_empty() { None } else { Some(volumes) },
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

pub fn build_service(dash: &RerunDashboard) -> Service {
    let name = dash.name_any();
    let lbls = labels(&name);
    let rendered = render_launch(dash);

    let mut ports: Vec<ServicePort> = Vec::new();
    if let Some(wp) = rendered.web_port {
        ports.push(ServicePort {
            name: Some("web".into()),
            port: wp as i32,
            target_port: Some(IntOrString::Int(wp as i32)),
            protocol: Some("TCP".into()),
            ..Default::default()
        });
    }
    if let Some(lp) = rendered.live_port {
        ports.push(ServicePort {
            name: Some("ingest".into()),
            port: lp as i32,
            target_port: Some(IntOrString::Int(lp as i32)),
            protocol: Some("TCP".into()),
            ..Default::default()
        });
    }

    Service {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(name.clone()),
            namespace: dash.metadata.namespace.clone(),
            labels: Some(lbls.clone()),
            owner_references: Some(vec![owner_ref(dash)]),
            ..Default::default()
        },
        spec: Some(ServiceSpec {
            type_: Some("ClusterIP".into()),
            selector: Some(lbls),
            ports: if ports.is_empty() { None } else { Some(ports) },
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Compute the in-cluster DNS hostname of the viewer service.
pub fn service_dns(dash: &RerunDashboard) -> String {
    let name = dash.name_any();
    let ns = dash.metadata.namespace.clone().unwrap_or_default();
    format!("{name}.{ns}.svc.cluster.local")
}
