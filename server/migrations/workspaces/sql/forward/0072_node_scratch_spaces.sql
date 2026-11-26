SET LOCAL aidash.scope = 'deployment';
INSERT INTO "scratch_spaces" ("id", "tenant", "node", "name", "creator", "state", "is_default", "created_at", "last_used_at")
SELECT gen_random_uuid(), "tenant", "name", 'Node Scratch Space', '', 'active', TRUE, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP FROM "nodes";
INSERT INTO "scratch_space_settings" ("space_id", "tenant", "settings")
SELECT "id", "tenant", '{"allowed_hosts":{"hosts":["api.github.com","api.nuget.org","archive.ubuntu.com","auth.docker.io","builds.hex.pm","cdn.quay.io","cloud.r-project.org","codeload.github.com","conda.anaconda.org","crates.io","deb.debian.org","dl-cdn.alpinelinux.org","downloads.gradle.org","files.pythonhosted.org","getcomposer.org","ghcr.io","github.com","hex.pm","index.crates.io","index.rubygems.org","jsr.io","npm.jsr.io","objects.githubusercontent.com","packagist.org","pkg-containers.githubusercontent.com","plugins.gradle.org","ports.ubuntu.com","production.cloudflare.docker.com","proxy.golang.org","pub.dev","pypi.org","quay.io","registry-1.docker.io","registry.npmjs.org","registry.yarnpkg.com","release-assets.githubusercontent.com","releases.astral.sh","repo.hex.pm","repo.maven.apache.org","repo.packagist.org","repo.yarnpkg.com","repo1.maven.org","rubygems.org","security.debian.org","security.ubuntu.com","services.gradle.org","static.crates.io","static.rust-lang.org","sum.golang.org"],"kind":"hosts"},"capacity_bytes":536870912000,"cpu_millis":8000,"daily_received_bytes":107374182400,"daily_sent_bytes":107374182400,"docker_enabled":false,"execution_image":null,"external_services":[],"memory_bytes":34359738368}' FROM "scratch_spaces" WHERE "is_default";
UPDATE "threads" SET "scratch_space_id" = (
    SELECT "scratch_spaces"."id" FROM "scratch_spaces"
    WHERE "scratch_spaces"."tenant" = "threads"."tenant"
      AND "scratch_spaces"."node" = "threads"."node"
      AND "scratch_spaces"."is_default"
);
DELETE FROM "scratch_spaces" WHERE NOT "is_default";
