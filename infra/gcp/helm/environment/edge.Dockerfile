FROM ubuntu:24.04
RUN apt-get update && apt-get install -y --no-install-recommends nginx libnginx-mod-http-lua lua-cjson ca-certificates \
    && rm -rf /var/lib/apt/lists/* /etc/nginx/sites-enabled/default
CMD ["nginx", "-g", "daemon off;"]
