FROM node:24.12.0-bookworm-slim AS build
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --ignore-scripts
COPY index.html tsconfig.json vite.config.ts ./
COPY src ./src
RUN npm run build

FROM nginxinc/nginx-unprivileged:1.31-alpine3.24
ENV AUTHENTICATION_BACKEND_URL=http://authentication:8081
ENV NGINX_ENVSUBST_FILTER=AUTHENTICATION_BACKEND_URL
ENV NGINX_ENVSUBST_OUTPUT_DIR=/tmp/nginx-conf
USER root
RUN sed -i 's|/etc/nginx/conf.d/\*.conf|/tmp/nginx-conf/*.conf|' /etc/nginx/nginx.conf && \
    rm /etc/nginx/conf.d/default.conf
COPY nginx.conf.template /etc/nginx/templates/default.conf.template
COPY --from=build /app/dist /usr/share/nginx/html
COPY --chmod=755 scripts/nginx-entrypoint.sh /usr/local/bin/auth-web-entrypoint
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/auth-web-entrypoint"]
CMD ["nginx", "-g", "daemon off;"]
EXPOSE 8080
