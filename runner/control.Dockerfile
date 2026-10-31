# Shared trusted image for the Runner, node guard and upstream installer.
# It runs privileged with host PID, containerd and host-root access, so a
# rebuild must not pick up different binaries: the base is pinned by digest and
# packages (curl, util-linux's nsenter) come from one immutable, signed Debian
# archive snapshot at exact versions. Update them together in a reviewed change.
FROM python:3.13-slim-bookworm@sha256:2325bb286ec344af3e5898cc224b5844e2707ac6e26b1632516fd3edc84a5e26
ARG TARGETARCH
ARG KUBECTL_VERSION=v1.34.0
ARG DEBIAN_SNAPSHOT=20261001T000000Z
# Snapshot Release files keep their original Valid-Until; the signature still
# authenticates every index and package.
RUN printf '%s\n' 'Types: deb' "URIs: https://snapshot.debian.org/archive/debian/${DEBIAN_SNAPSHOT}" \
       'Suites: bookworm bookworm-updates' 'Components: main' \
       'Signed-By: /usr/share/keyrings/debian-archive-keyring.gpg' '' \
       'Types: deb' "URIs: https://snapshot.debian.org/archive/debian-security/${DEBIAN_SNAPSHOT}" \
       'Suites: bookworm-security' 'Components: main' \
       'Signed-By: /usr/share/keyrings/debian-archive-keyring.gpg' > /etc/apt/sources.list.d/debian.sources \
    && apt-get -o Acquire::Check-Valid-Until=false update \
    && apt-get install -y --no-install-recommends \
       ca-certificates=20250419~deb12u1 curl=7.88.1-10+deb12u15 util-linux=2.38.1-5+deb12u3 \
    && curl -fsSL "https://dl.k8s.io/release/${KUBECTL_VERSION}/bin/linux/${TARGETARCH}/kubectl" -o /usr/local/bin/kubectl \
    && curl -fsSL "https://dl.k8s.io/release/${KUBECTL_VERSION}/bin/linux/${TARGETARCH}/kubectl.sha256" -o /tmp/kubectl.sha256 \
    && echo "$(cat /tmp/kubectl.sha256)  /usr/local/bin/kubectl" | sha256sum --check \
    && chmod 755 /usr/local/bin/kubectl && rm -rf /var/lib/apt/lists/* /tmp/kubectl.sha256
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
# The pinned gVisor release ships in the image, verified against gvisor.json, so
# a new or recreated Cluster Node installs it without reaching Google's release
# bucket (#394). Only these two files invalidate the downloaded layer.
COPY gvisor.json gvisor_installer.py /opt/aidash/
RUN python3 -I /opt/aidash/gvisor_installer.py bundle "${TARGETARCH}"
COPY control.py node_guard.py allocation_usage.py runner_journal.py /opt/aidash/
ENTRYPOINT ["python3", "-I", "/opt/aidash/control.py"]
