# Shared trusted image for the Runner, node guard and upstream installer.
FROM python:3.13-slim-bookworm@sha256:2325bb286ec344af3e5898cc224b5844e2707ac6e26b1632516fd3edc84a5e26
ARG TARGETARCH
ARG KUBECTL_VERSION=v1.34.0
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl util-linux \
    && curl -fsSL "https://dl.k8s.io/release/${KUBECTL_VERSION}/bin/linux/${TARGETARCH}/kubectl" -o /usr/local/bin/kubectl \
    && curl -fsSL "https://dl.k8s.io/release/${KUBECTL_VERSION}/bin/linux/${TARGETARCH}/kubectl.sha256" -o /tmp/kubectl.sha256 \
    && echo "$(cat /tmp/kubectl.sha256)  /usr/local/bin/kubectl" | sha256sum --check \
    && chmod 755 /usr/local/bin/kubectl && rm -rf /var/lib/apt/lists/* /tmp/kubectl.sha256
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
COPY control.py node_guard.py gvisor_installer.py /opt/aidash/
ENTRYPOINT ["python3", "-I", "/opt/aidash/control.py"]
