FROM ubuntu:24.04
# The packaged nginx.conf logs to files; send them to the kubelet-rotated
# container streams so the writable layer cannot grow without bound.
# curl lets the controller toggle admission through `kubectl exec` on loopback.
RUN apt-get update && apt-get install -y --no-install-recommends nginx libnginx-mod-http-lua lua-cjson ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* /etc/nginx/sites-enabled/default \
    && ln -sf /dev/stdout /var/log/nginx/access.log && ln -sf /dev/stderr /var/log/nginx/error.log
CMD ["nginx", "-g", "daemon off;"]
