//! RerunDashboard CRD — declarative Rerun (rerun.io) viewer dashboards.
//!
//! API group: rerun.nixlab.io, version v1alpha1.
//!
//! A RerunDashboard describes a long-lived Rerun web viewer plus its blueprint
//! (view tree), data source, optional PVC-backed `.rrd` replay, and public
//! exposure. Training jobs (e.g. SkyPilot tasks) attach by labeling their pod
//! `rerun.nixlab.io/dashboard: <name>`; an admission webhook injects
//! `RERUN_ENDPOINT` and `RERUN_APPLICATION_ID` env vars. The dashboard
//! outlives any single logger.
//!
//! Wire protocol notes (post Rerun 0.23):
//! - Rerun 0.23 (April 2025) removed the TCP/WebSocket wire transport.
//!   The viewer now speaks gRPC-over-HTTP on the historical TCP port (9876).
//! - `rr.connect_tcp()` is gone. The only logger API is
//!   `rr.connect_grpc(url="rerun+http://host:9876/proxy")`.
//! - The legacy 0.22.x WebSocket transport (port 9877) is still encodable in
//!   this CRD via `WireTransport::WebSocket`, but is only valid against a
//!   `rerunVersion` of `0.22.x`. The apiserver does not enforce this. The
//!   reconciler currently falls back to the gRPC port for invalid combos
//!   (see `resolve_live_port`) — a follow-up should also surface a
//!   Degraded condition on the dashboard status.

use k8s_openapi::api::core::v1::ResourceRequirements;
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
    printcolumn = r#"{"name":"Visibility","type":"string","jsonPath":".spec.presentation.ingress.visibility"}"#,
    printcolumn = r#"{"name":"Age","type":"date","jsonPath":".metadata.creationTimestamp"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct RerunDashboardSpec {
    /// Rerun SDK version pinning the viewer image (e.g. "0.31.3"). The viewer
    /// container `pip install`s `rerun-sdk==<version>` at startup.
    #[serde(default = "default_rerun_version")]
    pub rerun_version: String,

    /// Optional logical application id embedded into the viewer and
    /// injected into attached loggers. Defaults to the CR name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_id: Option<String>,

    /// Declarative blueprint: the view tree rendered in the viewer.
    pub blueprint: Blueprint,

    /// Where the viewer's data comes from (live streams, file replay, or both).
    pub data_source: DataSource,

    /// How the viewer is exposed (web UI, ingress).
    #[serde(default)]
    pub presentation: Presentation,

    /// Resource requests/limits for the viewer pod.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceRequirements>,
}

fn default_rerun_version() -> String {
    "0.31.3".to_string()
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
    let mut obj = schemars::schema::SchemaObject {
        instance_type: Some(schemars::schema::InstanceType::Object.into()),
        ..Default::default()
    };
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
// DataSource
// -----------------------------------------------------------------------------

/// Where the viewer's data comes from. Replaces the 0.22-era `IngestConfig`,
/// which conflated "loggers stream in" with "viewer is exposed".
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DataSource {
    /// `liveStream` (default): viewer accepts streaming data from loggers.
    /// `fileReplay`: viewer loads `.rrd` files from a PVC at startup.
    /// `mixed`: both.
    #[serde(default)]
    pub kind: DataSourceKind,

    /// LiveStream / Mixed only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<LiveStreamConfig>,

    /// FileReplay / Mixed only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<FileReplayConfig>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum DataSourceKind {
    #[default]
    LiveStream,
    FileReplay,
    Mixed,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct LiveStreamConfig {
    /// Wire transport the viewer accepts from loggers. Defaults to `grpc`
    /// (the only protocol Rerun 0.23+ supports).
    #[serde(default)]
    pub transport: WireTransport,

    /// Port the viewer listens on for logger data. When unset, the reconciler
    /// picks a version-aware default via `resolve_live_port()`:
    /// - 0.22.x + WebSocket → 9877
    /// - anything else → 9876 (gRPC)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,

    /// Server-side memory cap (e.g. "25%", "2GiB"). Maps to the viewer's
    /// `--server-memory-limit` CLI flag / `server_memory_limit` SDK arg.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_limit: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileReplayConfig {
    /// Reference to a PVC holding `.rrd` (and optionally `.rbl`) files.
    /// The PVC must be pre-provisioned — the operator does not synthesize one.
    pub pvc: String,

    /// Glob patterns (relative to the PVC mount) selecting files to replay.
    /// Default when empty: `["*.rrd"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub globs: Vec<String>,

    /// Mount path inside the viewer pod. Default: `/var/lib/rerun/recordings`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount_path: Option<String>,

    /// How long to retain `.rrd` files on the PVC, in seconds. Enforcement is
    /// reconciler-side (out-of-band reaper). `None` = keep forever.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_retention_seconds: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum WireTransport {
    /// 0.23+ gRPC-over-HTTP on port 9876. The only wire protocol Rerun
    /// supports today. Logger URL form: `rerun+http://host:9876/proxy`.
    #[default]
    Grpc,
    /// 0.22.x WebSocket ingest on port 9877. Legacy; valid only when
    /// `spec.rerunVersion` is `0.22.x`. The apiserver does not enforce this.
    /// The reconciler picks gRPC silently for invalid combos.
    WebSocket,
}

// -----------------------------------------------------------------------------
// Presentation
// -----------------------------------------------------------------------------

/// How the viewer is exposed to humans.
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Presentation {
    /// Host the web viewer. Default true.
    #[serde(default = "default_true")]
    pub web: bool,

    /// Port the web UI is served on. `None` ⇒ reconciler uses 9090
    /// (Rerun's default across all versions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_port: Option<u16>,

    /// Ingress / Service exposure rules.
    #[serde(default)]
    pub ingress: IngressConfig,
}

impl Default for Presentation {
    fn default() -> Self {
        Self {
            web: true,
            web_port: None,
            ingress: IngressConfig::default(),
        }
    }
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
// Port resolution
// =============================================================================

/// Default web UI port. Rerun has used 9090 across all 0.x releases.
pub const DEFAULT_WEB_PORT: u16 = 9090;

/// Default gRPC live ingest port (0.23+).
pub const DEFAULT_GRPC_PORT: u16 = 9876;

/// Default WebSocket live ingest port (0.22.x legacy).
pub const DEFAULT_WS_PORT: u16 = 9877;

/// Resolve the live-stream ingest port given a Rerun version and transport.
///
/// Rules:
/// - `0.22.x` + `WebSocket` → 9877 (ws_port in `serve_web`).
/// - Anything else → 9876 (gRPC HTTP-2 on the historical TCP port).
///
/// Note: an invalid combination (e.g. `WebSocket` against `0.23.0`) is not
/// flagged here — we silently pick the gRPC port, and the reconciler is
/// responsible for surfacing a Degraded condition. This function is total.
pub fn resolve_live_port(version: &str, transport: WireTransport) -> u16 {
    if transport == WireTransport::WebSocket && is_022(version) {
        DEFAULT_WS_PORT
    } else {
        DEFAULT_GRPC_PORT
    }
}

fn is_022(version: &str) -> bool {
    version == "0.22" || version.starts_with("0.22.")
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

    /// Bytes of `.rrd` currently on the PVC. Always `None` for v1alpha1 — the
    /// operator no longer synthesizes PVCs and does not measure them. Field
    /// is kept for forward compatibility.
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
    /// Web viewer URL, e.g. `http://rerun-foo.rerun.svc.cluster.local:9090`.
    pub web: String,
    /// Logger ingest URL. For gRPC: `rerun+http://host:9876/proxy`.
    /// For legacy WebSocket: `ws://host:9877`. Value is what the operator
    /// injects as `RERUN_ENDPOINT` into attached pods.
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
            rerun_version: "0.31.3".to_string(),
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
            data_source: DataSource {
                kind: DataSourceKind::LiveStream,
                live: Some(LiveStreamConfig {
                    transport: WireTransport::Grpc,
                    port: None,
                    memory_limit: Some("25%".to_string()),
                }),
                files: None,
            },
            presentation: Presentation {
                web: true,
                web_port: None,
                ingress: IngressConfig {
                    visibility: Visibility::Public,
                    service_type: ServiceType::ClusterIP,
                    public_hostname: Some("rerun.casazza.io".to_string()),
                },
            },
            resources: None,
        }
    }

    #[test]
    fn spec_serialization_roundtrip() {
        let spec = sample_spec();
        let s = serde_json::to_string(&spec).unwrap();
        let back: RerunDashboardSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(back.rerun_version, "0.31.3");
        assert_eq!(back.data_source.kind, DataSourceKind::LiveStream);
        assert_eq!(
            back.data_source.live.as_ref().unwrap().transport,
            WireTransport::Grpc
        );
        assert_eq!(back.presentation.ingress.visibility, Visibility::Public);
        assert_eq!(back.application_id.as_deref(), Some("spot_training"));
    }

    #[test]
    fn camel_case_field_names() {
        let spec = sample_spec();
        let v: Value = serde_json::to_value(&spec).unwrap();
        assert!(v.get("rerunVersion").is_some());
        assert!(v.get("applicationId").is_some());
        assert!(v.get("dataSource").is_some());
        assert!(v.get("presentation").is_some());

        let ds = v.get("dataSource").unwrap();
        assert_eq!(ds.get("kind").and_then(Value::as_str), Some("liveStream"));
        let live = ds.get("live").unwrap();
        assert_eq!(live.get("transport").and_then(Value::as_str), Some("grpc"));
        assert!(live.get("memoryLimit").is_some());

        let pres = v.get("presentation").unwrap();
        let ingress = pres.get("ingress").unwrap();
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
            },
            "dataSource": { "kind": "liveStream" }
        });
        let spec: RerunDashboardSpec = serde_json::from_value(j).unwrap();
        assert_eq!(spec.rerun_version, "0.31.3");
        assert_eq!(spec.data_source.kind, DataSourceKind::LiveStream);
        assert!(spec.data_source.live.is_none());
        assert!(spec.data_source.files.is_none());
        assert!(spec.presentation.web);
        assert!(spec.presentation.web_port.is_none());
        assert_eq!(spec.presentation.ingress.visibility, Visibility::Cluster);
        assert!(spec.blueprint.collapse_panels);
        match spec.blueprint.root {
            View::TimeSeries(v) => assert_eq!(v.origin, "/metrics"),
            _ => panic!("expected TimeSeries root"),
        }
    }

    #[test]
    fn deserialize_file_replay_spec() {
        let j = json!({
            "blueprint": {
                "root": { "kind": "spatial3D", "origin": "/" }
            },
            "dataSource": {
                "kind": "fileReplay",
                "files": {
                    "pvc": "training-recordings",
                    "globs": ["session-*.rrd", "*.rbl"],
                    "mountPath": "/data/rrds",
                    "fileRetentionSeconds": 604800
                }
            }
        });
        let spec: RerunDashboardSpec = serde_json::from_value(j).unwrap();
        assert_eq!(spec.data_source.kind, DataSourceKind::FileReplay);
        let files = spec.data_source.files.as_ref().expect("files set");
        assert_eq!(files.pvc, "training-recordings");
        assert_eq!(files.globs, vec!["session-*.rrd", "*.rbl"]);
        assert_eq!(files.mount_path.as_deref(), Some("/data/rrds"));
        assert_eq!(files.file_retention_seconds, Some(604800));

        // Roundtrip the FileReplay shape.
        let s = serde_json::to_string(&spec).unwrap();
        let back: RerunDashboardSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(back.data_source.kind, DataSourceKind::FileReplay);
        let v: Value = serde_json::to_value(&back).unwrap();
        let ds = v.get("dataSource").unwrap();
        assert_eq!(ds.get("kind").and_then(Value::as_str), Some("fileReplay"));
        let files = ds.get("files").unwrap();
        assert_eq!(files.get("pvc").and_then(Value::as_str), Some("training-recordings"));
        assert!(files.get("mountPath").is_some());
        assert!(files.get("fileRetentionSeconds").is_some());
    }

    #[test]
    fn deserialize_mixed_spec() {
        let j = json!({
            "blueprint": {
                "root": { "kind": "spatial3D", "origin": "/" }
            },
            "dataSource": {
                "kind": "mixed",
                "live": { "transport": "grpc" },
                "files": { "pvc": "rrds" }
            }
        });
        let spec: RerunDashboardSpec = serde_json::from_value(j).unwrap();
        assert_eq!(spec.data_source.kind, DataSourceKind::Mixed);
        assert!(spec.data_source.live.is_some());
        assert!(spec.data_source.files.is_some());

        let v: Value = serde_json::to_value(&spec).unwrap();
        let ds = v.get("dataSource").unwrap();
        assert_eq!(ds.get("kind").and_then(Value::as_str), Some("mixed"));
    }

    #[test]
    fn default_values() {
        assert_eq!(DashboardPhase::default(), DashboardPhase::Pending);
        assert_eq!(WireTransport::default(), WireTransport::Grpc);
        assert_eq!(DataSourceKind::default(), DataSourceKind::LiveStream);
        assert_eq!(Visibility::default(), Visibility::Cluster);
        assert_eq!(ServiceType::default(), ServiceType::ClusterIP);
        let status = RerunDashboardStatus::default();
        assert_eq!(status.phase, DashboardPhase::Pending);
        assert_eq!(status.connected_loggers, 0);
        assert!(status.endpoints.is_none());
        assert!(status.conditions.is_empty());
        assert!(status.persisted_bytes.is_none());
    }

    #[test]
    fn resolve_live_port_versions() {
        // 0.22.x WebSocket → 9877
        assert_eq!(
            resolve_live_port("0.22.0", WireTransport::WebSocket),
            DEFAULT_WS_PORT
        );
        assert_eq!(
            resolve_live_port("0.22.1", WireTransport::WebSocket),
            DEFAULT_WS_PORT
        );
        assert_eq!(
            resolve_live_port("0.22", WireTransport::WebSocket),
            DEFAULT_WS_PORT
        );

        // 0.22.x gRPC → 9876 (still gRPC)
        assert_eq!(
            resolve_live_port("0.22.1", WireTransport::Grpc),
            DEFAULT_GRPC_PORT
        );

        // 0.23+ → 9876 regardless of transport (WebSocket invalid; reconciler
        // surfaces the error, this fn is total).
        assert_eq!(
            resolve_live_port("0.23.0", WireTransport::Grpc),
            DEFAULT_GRPC_PORT
        );
        assert_eq!(
            resolve_live_port("0.23.0", WireTransport::WebSocket),
            DEFAULT_GRPC_PORT
        );
        assert_eq!(
            resolve_live_port("0.31.3", WireTransport::Grpc),
            DEFAULT_GRPC_PORT
        );
        assert_eq!(
            resolve_live_port("0.23.0-rc.1", WireTransport::WebSocket),
            DEFAULT_GRPC_PORT
        );

        // Spurious version that merely contains "0.22" should not match.
        assert_eq!(
            resolve_live_port("0.220.0", WireTransport::WebSocket),
            DEFAULT_GRPC_PORT
        );
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
