# Kubernetes tier

`infra/k8s/base` is a kustomize base containing:
- `app-api` and `app-worker` Deployments;
- a Service;
- an HPA and a PodDisruptionBudget;
- a NetworkPolicy;
- a ConfigMap.

Secrets are created out of band; `secret.example.yaml` shows the keys.

```sh
kubectl create secret generic app-secrets --from-env-file=prod-secrets.env   # or external-secrets
cd infra/k8s/base && kustomize edit set image app=registry.example.com/app:abc123
kubectl apply -k infra/k8s/base
```

| concern | how |
|---|---|
| probes | startup and liveness: `/healthz`; readiness: `/readyz` on the **ops port** (9090) |
| rollout | `maxUnavailable: 0`, `maxSurge: 1`; `terminationGracePeriodSeconds: 45` covers drain and shutdown |
| scaling | HPA on CPU 70%, 2–20 replicas, 5-minute scale-down stabilisation |
| availability | PDB `minAvailable: 1`; topology spread across nodes |
| security | non-root (65532), read-only root filesystem, all capabilities dropped, `RuntimeDefault` seccomp, no service account token |
| network | only the ingress controller namespace reaches 8080; only `monitoring` reaches 9090/9091 |
| metrics | `prometheus.io/*` annotations on the ops ports |
| migrations | on API start; concurrent replicas are serialised by sqlx's advisory lock |
| jobs | `app-worker` Deployment; `APP__JOBS__RUN_IN_PROCESS=false` in the API |

Overlays (production, staging) patch the ConfigMap origins, the replica counts and the resources.
Optional modules are enabled by their `APP__*` variables, pointing at managed or in-cluster
services:
- `APP__CACHE__BACKEND=redis`;
- `APP__MESSAGING__ENABLED=true`;
- `APP__ANALYTICS__ENABLED=true`.

Verification without a cluster: `infra/verify.sh` validates every file and the rendered
kustomization with `kubeconform -strict`. On 2026-10-04: 8 files, 7 rendered resources, all valid.
