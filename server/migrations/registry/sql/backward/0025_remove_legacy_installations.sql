CREATE TABLE public.installations (
    id text NOT NULL,
    version text NOT NULL,
    digest text NOT NULL,
    config jsonb NOT NULL,
    installed_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT installations_config CHECK (COALESCE((jsonb_typeof(config) = 'object'::text), false))
);
CREATE TABLE public.packages (
    id text NOT NULL,
    version text NOT NULL,
    manifest jsonb NOT NULL,
    digest text NOT NULL,
    manifest_source text DEFAULT ''::text NOT NULL,
    CONSTRAINT packages_digest CHECK (COALESCE((public.aidash_package_source_matches(manifest, manifest_source) AND (digest = ('sha256:'::text || encode(sha256(convert_to(manifest_source, 'UTF8'::name)), 'hex'::text)))), false)),
    CONSTRAINT packages_semver CHECK (COALESCE(((version ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'::text) AND
CASE
    WHEN (version ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'::text) THEN (((split_part(version, '.'::text, 1))::numeric <= '18446744073709551615'::numeric) AND ((split_part(version, '.'::text, 2))::numeric <= '18446744073709551615'::numeric) AND ((split_part(split_part(split_part(version, '.'::text, 3), '-'::text, 1), '+'::text, 1))::numeric <= '18446744073709551615'::numeric))
    ELSE false
END), false))
);
ALTER TABLE ONLY public.installations
    ADD CONSTRAINT installations_pkey PRIMARY KEY (id, version);
ALTER TABLE ONLY public.packages
    ADD CONSTRAINT packages_pkey PRIMARY KEY (id, version);
ALTER TABLE public.packages
    ADD CONSTRAINT packages_identity CHECK (public.aidash_binding_package_is_valid(manifest, id, version)) NOT VALID;
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.packages FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER marketplace_overlay_fence BEFORE INSERT OR UPDATE ON public.installations FOR EACH ROW EXECUTE FUNCTION public.marketplace_overlay_fence();
CREATE TRIGGER installations_registry_writes_lock BEFORE INSERT OR DELETE OR UPDATE ON public.installations FOR EACH STATEMENT EXECUTE FUNCTION public.lock_registry_installation_writes();
CREATE TRIGGER installations_config_guard BEFORE INSERT OR UPDATE OF id, version, config ON public.installations FOR EACH ROW EXECUTE FUNCTION public.guard_installation_config();
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.installations FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER registry_installations_model_override_guard AFTER DELETE ON public.registry FOR EACH ROW EXECUTE FUNCTION public.validate_registry_installations();
CREATE TRIGGER registry_installations_config_guard AFTER UPDATE OF id, version, kind, metadata ON public.registry FOR EACH ROW EXECUTE FUNCTION public.validate_registry_installations();
CREATE TRIGGER registry_installation_writes_lock BEFORE INSERT OR DELETE OR UPDATE ON public.registry FOR EACH STATEMENT EXECUTE FUNCTION public.lock_registry_installation_writes();
