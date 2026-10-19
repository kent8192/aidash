FROM python:3.13-slim-bookworm@sha256:2325bb286ec344af3e5898cc224b5844e2707ac6e26b1632516fd3edc84a5e26
ARG TARGETARCH
# Official release artifacts, pinned by version and SHA-256 (per architecture).
ARG NODE_VERSION=22.23.3
ARG NODE_SHA256_AMD64=df450af89261115ef9f9e3830c3eeb2cc9213b63c720b1af623cb5dcbe2e02de
ARG NODE_SHA256_ARM64=a44aeb94849a299b22df10b9e622ec2f605c2183501bc40590705131de7c740f
ARG UV_VERSION=0.13.1
ARG UV_SHA256_AMD64=eadd9d27c134ab5ac1ffd4473b97a741407f3ea238288f03121fc4deddf90220
ARG UV_SHA256_ARM64=9041c73c99fdc5892cf128140beb354eca9231d502068cc36e6cb53a8e59ad14
# Node.js LTS (npm, npx) and uv (uvx) run against the image's Python only.
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends ca-certificates curl xz-utils; \
    case "${TARGETARCH}" in \
        amd64) node_arch=x64; node_sha="${NODE_SHA256_AMD64}"; uv_arch=x86_64; uv_sha="${UV_SHA256_AMD64}" ;; \
        arm64) node_arch=arm64; node_sha="${NODE_SHA256_ARM64}"; uv_arch=aarch64; uv_sha="${UV_SHA256_ARM64}" ;; \
        *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    mkdir -p /tmp/download /opt/node; \
    curl -fsSLo /tmp/download/node.tar.xz "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${node_arch}.tar.xz"; \
    echo "${node_sha}  /tmp/download/node.tar.xz" | sha256sum -c -; \
    tar -xJf /tmp/download/node.tar.xz -C /opt/node --strip-components=1 --no-same-owner; \
    ln -s /opt/node/bin/node /usr/local/bin/node; \
    ln -s /opt/node/bin/npm /usr/local/bin/npm; \
    ln -s /opt/node/bin/npx /usr/local/bin/npx; \
    curl -fsSLo /tmp/download/uv.tar.gz "https://github.com/astral-sh/uv/releases/download/${UV_VERSION}/uv-${uv_arch}-unknown-linux-gnu.tar.gz"; \
    echo "${uv_sha}  /tmp/download/uv.tar.gz" | sha256sum -c -; \
    tar -xzf /tmp/download/uv.tar.gz -C /tmp/download --strip-components=1; \
    install -m 0755 /tmp/download/uv /tmp/download/uvx /usr/local/bin/; \
    rm -rf /tmp/download; \
    apt-get purge -y --auto-remove curl xz-utils; \
    rm -rf /var/lib/apt/lists/*
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
ENV UV_PYTHON_DOWNLOADS=never UV_PYTHON=/usr/local/bin/python
COPY requirements.lock /opt/aidash/requirements.lock
RUN pip install --no-cache-dir --require-hashes -r /opt/aidash/requirements.lock
COPY sandbox.py collector.py kernel.py python_cells.py probe.py extract.py install.py overlay.py /opt/aidash/
WORKDIR /work
USER 10000:10000
ENTRYPOINT ["python", "-I", "/opt/aidash/sandbox.py"]
