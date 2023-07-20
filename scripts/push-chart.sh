#!/bin/sh
TAG=$(git describe | sed 's/^v//')
helm package --version $TAG --app-version $TAG helm
helm push prometheus-ping-$TAG.tgz oci://ghcr.io/remram44/prometheus-ping/helm-charts
