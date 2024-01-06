#!/bin/sh
TAG=$(git describe | sed 's/^v//')
docker buildx build --pull \
    . \
    --platform linux/amd64 \
    --push --tag ghcr.io/remram44/prometheus-ping:$TAG
