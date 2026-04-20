//! RerunDashboard CRD — declarative Rerun (rerun.io) viewer dashboards.
//!
//! API group: rerun.nixlab.io, version v1alpha1.
//!
//! A RerunDashboard describes a long-lived Rerun web viewer plus its blueprint
//! (view tree), ingest endpoint, optional PVC-backed .rrd persistence, and
//! public exposure. Training jobs (e.g. SkyPilot tasks) attach by labeling
//! their pod `rerun.nixlab.io/dashboard: <name>`; an admission webhook injects
//! `RERUN_ENDPOINT` and `RERUN_APPLICATION_ID` env vars. The dashboard
//! outlives any single logger.
//!
//! Scoped to Rerun SDK 0.22.x (connect_tcp / connect_grpc / serve_web).

use k8s_openapi::api::core::v1::{PersistentVolumeClaimSpec, ResourceRequirements};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// RerunDashboard
// =============================================================================

#[derive(CustomResource, Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[kube(
    group = "rerun.nixlab.io",
    version = "v1alpha1",
    kind = "RerunDashboard",
    plural = "rerundashboards",
    singular = "rerundashboard",
    shortname = "rrd",
    namespaced,
    status = "RerunDashboardStatus",
    printcolumn = r#"{"name":"Phase","type":"string","jsonPath":".status.phase"}"#,
    printcolumn = r#"{"name":"Web","type":"string","jsonPath":".status.endpoints.web"}"#,
    printcolumn = r#"{"name":"Loggers","type":"integer","jsonPath":".status.connectedLoggers"}"#,
    printcolumn = r#"{"name":"Visibility","type":"string","jsonPath":".spec.ingress.visibility"}"#,
    printcolumn = r#"{"name":"Age","type":"date","jsonPath":".metadata.creationTimestamp"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct RerunDashboardSpec {
    /// Rerun SDK version to pin the viewer image to (e.g. "0.22.1").
    #[serde(default = "default_rerun_version")]
    pub rerun_version: String,

    /// Optional logical application id embedded into the viewer and
    /// injected into attached loggers. Defaults to the CR name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_id: Option<String>,

    /// Declarative blueprint: the view tree rendered in the viewer.
    pub blueprint: Blueprint,

    /// How data flows from loggers into the viewer.
    #[serde(default)]
    pub ingest: IngestConfig,

    /// Durability of streamed data.
    #[serde(default)]
    pub storage: StorageConfig,

    /// How long to retain persisted recordings. Only applies when
    /// `storage.mode = Persistent`. None = keep forever.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_seconds: Option<i64>,

    /// Viewer exposure.
    #[serde(default)]
    pub ingress: IngressConfig,

    /// Resource requests/limits for the viewer pod.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceRequirements>,
}

fn default_rerun_version() -> String {
    "0.22.1".to_string()
}

// -----------------------------------------------------------------------------
// Blueprint (declarative view tree)
// -----------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Blueprint {
    /// Root view node. The schema preserves unknown fields — validation of
    /// the view tree happens in the operator, not the apiserver, because
    /// kube-core's structural schema generator cannot encode
    /// internally-tagged enums where each variant contributes a distinct
    /// `kind` literal.
    #[schemars(schema_with = "preserve_unknown_fields")]
    pub root: View,

    /// Collapse the blueprint/selection/time panels for a cleaner embed.
    #[serde(default = "default_true")]
    pub collapse_panels: bool,
}

/// Emit a schema that tells the apiserver not to validate the subtree.
/// `View` is still typed in Rust — the operator deserializes it at reconcile
/// time — but Kubernetes accepts any JSON object here.
fn preserve_unknown_fields(
    _gen: &mut schemars::r#gen::SchemaGenerator,
) -> schemars::schema::Schema {
    let mut obj = schemars::schema::SchemaObject::default();
    obj.instance_type = Some(schemars::schema::InstanceType::Object.into());
    obj.extensions.insert(
        "x-kubernetes-preserve-unknown-fields".to_string(),
        serde_json::json!(true),
    );
    schemars::schema::Schema::Object(obj)
}

/// A node in the Rerun blueprint tree. Mirrors the subset of `rrb.*`
/// constructors actually used. Extend when a new view type is genuinely
/// needed; do not chase `rrb` completeness.
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum View {
    // --- Containers ---
    Horizontal(Container),
    Vertical(Container),
    Grid(GridContainer),

    // --- Leaf views (`rrb.*View`) ---
    Spatial3D(LeafView),
    Spatial2D(LeafView),
    TimeSeries(LeafView),
    TextLog(LeafView),
    BarChart(LeafView),
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Container {
    /// Child views, rendered left-to-right (Horizontal) or top-to-bottom (Vertical).
    pub children: Vec<View>,

    /// Relative size shares per child (maps to `row_shares` / `column_shares`).
    /// Length must equal `children.len()` or be empty (equal share).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shares: Vec<u32>,

    /// Optional display name for the container tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GridContainer {
    pub children: Vec<View>,

    /// Number of columns. Rows inferred from child count.
    #[serde(default = "default_grid_columns")]
    pub columns: u32,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

fn default_grid_columns() -> u32 {
    2
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LeafView {
    /// Entity path origin (e.g. "/", "/world/metrics").
    pub origin: String,

    /// Entity path filter expressions (e.g. ["+/**", "-/debug/**"]).
    /// Empty == include everything under `origin`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contents: Vec<String>,

    /// Optional display name for the view tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

// -----------------------------------------------------------------------------
// Ingest
// -----------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IngestConfig {
    /// Transport the viewer accepts from loggers.
    #[serde(default)]
    pub protocol: IngestProtocol,

    /// TCP/WebSocket ingest port (viewer listens here, loggers connect here).
    /// Defaults to Rerun's upstream 9877 (`ws_port` in `serve_web`).
    #[serde(default = "default_ingest_port")]
    pub port: u16,

    /// Viewer HTTP port for the web UI. Defaults to Rerun's upstream 9090.
    #[serde(default = "default_web_port")]
    pub web_port: u16,

    /// Cap the viewer's in-memory ring buffer. Maps to `server_memory_limit`
    /// (e.g. "25%", "2GiB"). Only meaningful when `storage.mode == Ephemeral`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_limit: Option<String>,
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            protocol: IngestProtocol::default(),
            port: default_ingest_port(),
            web_port: default_web_port(),
            memory_limit: None,
        }
    }
}

fn default_ingest_port() -> u16 {
    9877
}

fn default_web_port() -> u16 {
    9090
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default, PartialEq)]
pub enum IngestProtocol {
    /// `rr.connect_tcp(addr)` in the logger. Matches the current sidecar
    /// pattern where `serve_web`'s ws_port accepts streaming ingest.
    #[default]
    Tcp,
    /// `rr.connect_grpc(addr)` in the logger.
    Grpc,
}

// -----------------------------------------------------------------------------
// Storage
// -----------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct StorageConfig {
    #[serde(default)]
    pub mode: StorageMode,

    /// Reuse an existing PVC. Mutually exclusive with `claimTemplate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing_claim: Option<String>,

    /// Provision a new PVC from this template (StatefulSet-style).
    /// Mutually exclusive with `existingClaim`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_template: Option<PersistentVolumeClaimSpec>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default, PartialEq)]
pub enum StorageMode {
    /// Viewer ring buffer only; data lost on pod restart.
    #[default]
    Ephemeral,
    /// Viewer also writes `.rrd` recordings to a PVC for later replay.
    Persistent,
}

// -----------------------------------------------------------------------------
// Ingress
// -----------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct IngressConfig {
    #[serde(default)]
    pub visibility: Visibility,

    /// Service type for the viewer. Ignored when `visibility = Public` and a
    /// Cloudflare tunnel fronts the service.
    #[serde(default)]
    pub service_type: ServiceType,

    /// FQDN fronted by a Cloudflare tunnel. Required when `visibility = Public`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_hostname: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default, PartialEq)]
pub enum Visibility {
    /// Reachable only inside the cluster (ClusterIP Service).
    #[default]
    Cluster,
    /// Exposed on the public internet via Cloudflare tunnel + Access.
    Public,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default, PartialEq)]
pub enum ServiceType {
    #[default]
    ClusterIP,
    NodePort,
}

// =============================================================================
// Status
// =============================================================================

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct RerunDashboardStatus {
    #[serde(default)]
    pub phase: DashboardPhase,

    /// Reachable endpoints the operator has reconciled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoints: Option<Endpoints>,

    /// Current number of distinct loggers streaming into the viewer.
    #[serde(default)]
    pub connected_loggers: u32,

    /// Wall-clock time of the last event received (RFC 3339).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_time: Option<String>,

    /// Bytes of `.rrd` currently on the PVC (Persistent mode only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persisted_bytes: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_transition_time: Option<String>,

    /// Standard Kubernetes-style condition array.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct Endpoints {
    /// Web viewer URL, e.g. "http://rerun-foo.rerun.svc.cluster.local:9090/?url=ws://…".
    pub web: String,
    /// Logger ingest URL, e.g. "tcp://rerun-foo.rerun.svc.cluster.local:9877"
    /// or "grpc://…". Value is what the operator injects as `RERUN_ENDPOINT`
    /// into attached pods.
    pub ingest: String,
    /// Public URL if `visibility = Public`, else None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default, PartialEq)]
pub enum DashboardPhase {
    #[default]
    Pending,
    Provisioning,
    Ready,
    Degraded,
    Terminating,
    Error,
}

impl std::fmt::Display for DashboardPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DashboardPhase::Pending => write!(f, "Pending"),
            DashboardPhase::Provisioning => write!(f, "Provisioning"),
            DashboardPhase::Ready => write!(f, "Ready"),
            DashboardPhase::Degraded => write!(f, "Degraded"),
            DashboardPhase::Terminating => write!(f, "Terminating"),
            DashboardPhase::Error => write!(f, "Error"),
        }
    }
}

// =============================================================================
// helpers
// =============================================================================

fn default_true() -> bool {
    true
}

// =============================================================================
// tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn sample_spec() -> RerunDashboardSpec {
        RerunDashboardSpec {
            rerun_version: "0.22.1".to_string(),
            application_id: Some("spot_training".to_string()),
            blueprint: Blueprint {
                collapse_panels: true,
                root: View::Horizontal(Container {
                    children: vec![
                        View::Spatial3D(LeafView {
                            origin: "/".to_string(),
                            contents: vec!["+/**".to_string()],
                            name: Some("Robot".to_string()),
                        }),
                        View::Vertical(Container {
                            children: vec![
                                View::TimeSeries(LeafView {
                                    origin: "/world/metrics".to_string(),
                                    contents: vec!["+/**".to_string()],
                                    name: Some("Metrics".to_string()),
                                }),
                                View::TimeSeries(LeafView {
                                    origin: "/metrics".to_string(),
                                    contents: vec!["+/**".to_string()],
                                    name: Some("Training".to_string()),
                                }),
                            ],
                            shares: vec![],
                            name: None,
                        }),
                    ],
                    shares: vec![3, 1],
                    name: None,
                }),
            },
            ingest: IngestConfig {
                protocol: IngestProtocol::Tcp,
                port: 9877,
                web_port: 9090,
                memory_limit: Some("25%".to_string()),
            },
            storage: StorageConfig::default(),
            retention_seconds: None,
            ingress: IngressConfig {
                visibility: Visibility::Public,
                service_type: ServiceType::ClusterIP,
                public_hostname: Some("rerun.casazza.io".to_string()),
            },
            resources: None,
        }
    }

    #[test]
    fn spec_serialization_roundtrip() {
        let spec = sample_spec();
        let s = serde_json::to_string(&spec).unwrap();
        let back: RerunDashboardSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(back.rerun_version, "0.22.1");
        assert_eq!(back.ingest.port, 9877);
        assert_eq!(back.ingress.visibility, Visibility::Public);
        assert_eq!(back.application_id.as_deref(), Some("spot_training"));
    }

    #[test]
    fn camel_case_field_names() {
        let spec = sample_spec();
        let v: Value = serde_json::to_value(&spec).unwrap();
        assert!(v.get("rerunVersion").is_some());
        assert!(v.get("applicationId").is_some());
        assert!(v.get("retentionSeconds").is_none());
        let ingest = v.get("ingest").unwrap();
        assert!(ingest.get("webPort").is_some());
        assert!(ingest.get("memoryLimit").is_some());
        let ingress = v.get("ingress").unwrap();
        assert!(ingress.get("publicHostname").is_some());
        assert!(ingress.get("serviceType").is_some());
    }

    #[test]
    fn view_tagged_enum_serialization() {
        let view = View::Spatial3D(LeafView {
            origin: "/".to_string(),
            contents: vec!["+/**".to_string()],
            name: Some("Robot".to_string()),
        });
        let v: Value = serde_json::to_value(&view).unwrap();
        assert_eq!(v.get("kind").and_then(Value::as_str), Some("spatial3D"));
        assert_eq!(v.get("origin").and_then(Value::as_str), Some("/"));
    }

    #[test]
    fn deserialize_minimal_spec() {
        let j = json!({
            "blueprint": {
                "root": { "kind": "timeSeries", "origin": "/metrics" }
            }
        });
        let spec: RerunDashboardSpec = serde_json::from_value(j).unwrap();
        assert_eq!(spec.rerun_version, "0.22.1");
        assert_eq!(spec.ingest.port, 9877);
        assert_eq!(spec.ingest.web_port, 9090);
        assert_eq!(spec.ingest.protocol, IngestProtocol::Tcp);
        assert_eq!(spec.storage.mode, StorageMode::Ephemeral);
        assert_eq!(spec.ingress.visibility, Visibility::Cluster);
        assert!(spec.blueprint.collapse_panels);
        match spec.blueprint.root {
            View::TimeSeries(v) => assert_eq!(v.origin, "/metrics"),
            _ => panic!("expected TimeSeries root"),
        }
    }

    #[test]
    fn default_values() {
        assert_eq!(DashboardPhase::default(), DashboardPhase::Pending);
        assert_eq!(IngestProtocol::default(), IngestProtocol::Tcp);
        assert_eq!(StorageMode::default(), StorageMode::Ephemeral);
        assert_eq!(Visibility::default(), Visibility::Cluster);
        assert_eq!(ServiceType::default(), ServiceType::ClusterIP);
        let status = RerunDashboardStatus::default();
        assert_eq!(status.phase, DashboardPhase::Pending);
        assert_eq!(status.connected_loggers, 0);
        assert!(status.endpoints.is_none());
        assert!(status.conditions.is_empty());
    }

    #[test]
    fn crd_generation() {
        use kube::CustomResourceExt;
        let crd = RerunDashboard::crd();
        assert_eq!(
            crd.metadata.name.as_deref(),
            Some("rerundashboards.rerun.nixlab.io")
        );
    }
}
