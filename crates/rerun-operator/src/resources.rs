//! Build Kubernetes objects owned by a RerunDashboard.

use std::collections::BTreeMap;

use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec};
use k8s_openapi::api::core::v1::{
    ConfigMap, ConfigMapVolumeSource, Container, ContainerPort, PersistentVolumeClaim,
    PersistentVolumeClaimVolumeSource, PodSpec, PodTemplateSpec, Service, ServicePort, ServiceSpec,
    Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, OwnerReference};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::ResourceExt;
use rerun_operator_api::v1alpha1::{RerunDashboard, StorageMode};

use crate::blueprint;

pub const MANAGER: &str = "rerun-operator";
const BLUEPRINT_KEY: &str = "blueprint.py";
const BLUEPRINT_MOUNT: &str = "/opt/rerun/blueprint";
const CHECKPOINT_MOUNT: &str = "/var/lib/rerun";

/// Default viewer image: python:3.13-slim with rerun-sdk installed at start.
/// A future release should ship a purpose-built image.
const VIEWER_IMAGE: &str = "python:3.13-slim";

pub fn labels(dashboard_name: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("app.kubernetes.io/name".into(), "rerun-dashboard".into()),
        ("app.kubernetes.io/instance".into(), dashboard_name.into()),
        ("app.kubernetes.io/managed-by".into(), MANAGER.into()),
        (
            "rerun.nixlab.io/dashboard".into(),
            dashboard_name.into(),
        ),
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

pub fn build_configmap(dash: &RerunDashboard) -> ConfigMap {
    let name = dash.name_any();
    let app_id = dash
        .spec
        .application_id
        .clone()
        .unwrap_or_else(|| name.clone());
    let rendered = blueprint::render(&app_id, &dash.spec);

    ConfigMap {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(format!("{name}-blueprint")),
            namespace: dash.metadata.namespace.clone(),
            labels: Some(labels(&name)),
            owner_references: Some(vec![owner_ref(dash)]),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(BLUEPRINT_KEY.into(), rendered.python)])),
        ..Default::default()
    }
}

pub fn build_deployment(dash: &RerunDashboard) -> Deployment {
    let name = dash.name_any();
    let lbls = labels(&name);
    let ingest = &dash.spec.ingest;
    let version = &dash.spec.rerun_version;

    let install_cmd = format!(
        "pip install --no-cache-dir --quiet 'rerun-sdk=={version}' && \
         exec python -u {mount}/{key}",
        mount = BLUEPRINT_MOUNT,
        key = BLUEPRINT_KEY,
    );

    let mut volumes = vec![Volume {
        name: "blueprint".into(),
        config_map: Some(ConfigMapVolumeSource {
            name: format!("{name}-blueprint"),
            ..Default::default()
        }),
        ..Default::default()
    }];
    let mut mounts = vec![VolumeMount {
        name: "blueprint".into(),
        mount_path: BLUEPRINT_MOUNT.into(),
        read_only: Some(true),
        ..Default::default()
    }];

    if dash.spec.storage.mode == StorageMode::Persistent {
        let claim_name = dash
            .spec
            .storage
            .existing_claim
            .clone()
            .unwrap_or_else(|| format!("{name}-checkpoints"));
        volumes.push(Volume {
            name: "recordings".into(),
            persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
                claim_name,
                read_only: Some(false),
            }),
            ..Default::default()
        });
        mounts.push(VolumeMount {
            name: "recordings".into(),
            mount_path: CHECKPOINT_MOUNT.into(),
            ..Default::default()
        });
    }

    let container = Container {
        name: "viewer".into(),
        image: Some(VIEWER_IMAGE.into()),
        command: Some(vec!["sh".into(), "-c".into(), install_cmd]),
        ports: Some(vec![
            ContainerPort {
                name: Some("web".into()),
                container_port: ingest.web_port as i32,
                protocol: Some("TCP".into()),
                ..Default::default()
            },
            ContainerPort {
                name: Some("ingest".into()),
                container_port: ingest.port as i32,
                protocol: Some("TCP".into()),
                ..Default::default()
            },
        ]),
        volume_mounts: Some(mounts),
        resources: dash.spec.resources.clone(),
        ..Default::default()
    };

    Deployment {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(name.clone()),
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
                    volumes: Some(volumes),
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
    let ingest = &dash.spec.ingest;

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
            ports: Some(vec![
                ServicePort {
                    name: Some("web".into()),
                    port: ingest.web_port as i32,
                    target_port: Some(IntOrString::Int(ingest.web_port as i32)),
                    protocol: Some("TCP".into()),
                    ..Default::default()
                },
                ServicePort {
                    name: Some("ingest".into()),
                    port: ingest.port as i32,
                    target_port: Some(IntOrString::Int(ingest.port as i32)),
                    protocol: Some("TCP".into()),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Build a PVC from a claimTemplate. Only called when storage.mode = Persistent
/// AND spec.storage.claim_template is set AND existing_claim is None.
pub fn build_pvc(dash: &RerunDashboard) -> Option<PersistentVolumeClaim> {
    let template = dash.spec.storage.claim_template.as_ref()?;
    let name = dash.name_any();
    Some(PersistentVolumeClaim {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(format!("{name}-checkpoints")),
            namespace: dash.metadata.namespace.clone(),
            labels: Some(labels(&name)),
            owner_references: Some(vec![owner_ref(dash)]),
            ..Default::default()
        },
        spec: Some(template.clone()),
        ..Default::default()
    })
}

/// Compute the in-cluster DNS hostname of the viewer service.
pub fn service_dns(dash: &RerunDashboard) -> String {
    let name = dash.name_any();
    let ns = dash.metadata.namespace.clone().unwrap_or_default();
    format!("{name}.{ns}.svc.cluster.local")
}
