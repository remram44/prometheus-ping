Prometheus ping metrics
=======================

This system measures network latency between its instances. You can deploy it in multiple servers or data centers and collect the latency and packet loss information via Prometheus.

Each instance is given a **source name** (reported in the metrics), a **UDP port** (for other instances to ping), and a **list of targets** (that will be pinged every second).

For example:

```
python prometheus_ping.py --source spire:5000 --listen-port 5000 database:5555
python prometheus_ping.py --source database:5555 --listen-port 5555 spire:5000
```

The resulting metrics look like:

```
ping_packet_loss_total{source="spire:5000",target="database:5555"} 123.0
ping_packet_loss_total{source="database:5555",target="spire:5000"} 27.0
ping_latency_average_30s{source="spire:5000",target="database:5555"} 0.00123
ping_latency_average_30s{source="database:5555",target="spire:5000"} 0.00130
```

## Deploying with Helm

If you have 3 zones `alpha`, `bravo`, and `charlie`, you can deploy like this:

```
helm install prometheus-ping \
    oci://ghcr.io/remram44/prometheus-ping/helm-charts/prometheus-ping \
    --namespace default \
    --set-json 'locations.alpha={"topology.kubernetes.io/zone": "alpha"}' \
    --set-json 'locations.bravo={"topology.kubernetes.io/zone": "bravo"}' \
    --set-json 'locations.charlie={"topology.kubernetes.io/zone": "charlie"}'
```
