# This image enforces admission for all public traffic, so a rebuild must not
# pick up different Nginx/Lua code: the base is pinned by digest and every
# package comes from one immutable, signed Ubuntu archive snapshot at an exact
# version. Update the digest, snapshot and versions together in a reviewed
# change. The snapshot service publishes amd64 only, the platform images.py
# builds and publishes.
# The snapshot service is HTTPS-only and the base has no CA bundle; borrow one
# from the repository's other digest-pinned base. Package integrity still rests
# on the archive's signed InRelease and the exact versions below.
FROM python:3.13-slim-bookworm@sha256:2325bb286ec344af3e5898cc224b5844e2707ac6e26b1632516fd3edc84a5e26 AS certificates
FROM ubuntu:24.04@sha256:534baea6a22c03a63003dbc8dbe78fe34bc0d7e595d9a9dc9834884ff530eb55
COPY --from=certificates /etc/ssl/certs/ca-certificates.crt /etc/aidash-apt-ca.crt
# The packaged nginx.conf logs to files; send them to the kubelet-rotated
# container streams so the writable layer cannot grow without bound.
# curl lets the controller toggle admission through `kubectl exec` on loopback.
RUN printf '%s\n' 'Types: deb' 'URIs: https://snapshot.ubuntu.com/ubuntu/20261001T000000Z' \
       'Suites: noble noble-updates noble-security' 'Components: main universe' \
       'Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg' > /etc/apt/sources.list.d/ubuntu.sources \
    && printf 'Acquire::https::CAInfo "/etc/aidash-apt-ca.crt";\n' > /etc/apt/apt.conf.d/99aidash-ca \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
       nginx=1.24.0-2ubuntu7.18 libnginx-mod-http-lua=1:0.10.26-2 lua-cjson=2.1.0+dfsg-2.2 \
       ca-certificates=20260601~24.04.1 curl=8.5.0-2ubuntu10.15 \
    && rm -rf /var/lib/apt/lists/* /etc/apt/apt.conf.d/99aidash-ca /etc/aidash-apt-ca.crt /etc/nginx/sites-enabled/default \
    && ln -sf /dev/stdout /var/log/nginx/access.log && ln -sf /dev/stderr /var/log/nginx/error.log
CMD ["nginx", "-g", "daemon off;"]
