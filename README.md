# rerun-operator

Kubernetes operator that reconciles `RerunDashboard` custom resources into
running [Rerun](https://rerun.io) web viewers with declaratively-defined
blueprints.

## Why

Rerun ships a viewer binary and language SDKs but has no Kubernetes story. A
training job that wants a dashboard today has to embed a Rerun sidecar with
its blueprint hardcoded as Python in the pod spec. This operator lets you
declare a dashboard once as a CR, and have SkyPilot / Ray / plain Pods attach
to it by label — no sidecar plumbing per job, blueprint stays in source
control as structured data.

## API

`rerun.nixlab.io/v1alpha1/RerunDashboard` — see
`crates/rerun-operator-api/src/v1alpha1.rs` for the full schema.

```yaml
apiVersion: rerun.nixlab.io/v1alpha1
kind: RerunDashboard
metadata:
  name: spot-training
  namespace: hpc
spec:
  rerunVersion: "0.22.1"
  applicationId: spot_training
  blueprint:
    collapsePanels: true
    root:
      kind: horizontal
      children:
        - kind: spatial3D
          origin: "/"
          name: Robot
        - kind: vertical
          children:
            - kind: timeSeries
              origin: /world/metrics
              name: Metrics
            - kind: timeSeries
              origin: /metrics
              name: Training
      shares: [3, 1]
  ingress:
    visibility: public
    publicHostname: rerun.casazza.io
```

## Status

v0.1.0 — scaffolding. CRD types compile and round-trip. Reconciler and
admission webhook are not yet implemented.

## Development

```bash
nix develop
cargo test -p rerun-operator-api
cargo build -p rerun-operator
```

## Related

Shares telemetry and CLI scaffolding with
[hephaestus](https://github.com/casazza-info/hephaestus) via the
`hephaestus-operator-lib` crate.
