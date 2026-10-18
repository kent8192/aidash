SET LOCAL search_path = public, pg_catalog;
-- Every existing policy bundle gets a root record before the bundle references
-- it. Personal Tenants named after their identity keep their identity link, so
-- sign-in does not provision a second one.
INSERT INTO tenants (name, label, personal, created_at, created_by_kind, created_by_identity)
SELECT tenant, NULL, tenant LIKE 'personal-%', updated_at, 'deployment_administrator', NULL
FROM authorization_bundles
ON CONFLICT (name) DO NOTHING;
INSERT INTO personal_tenants (identity_id, tenant, created_at)
SELECT identity.id, root.name, root.created_at
FROM dashboard_identities AS identity
JOIN tenants AS root ON root.name = 'personal-' || replace(identity.id::text, '-', '')
ON CONFLICT (identity_id) DO NOTHING;
