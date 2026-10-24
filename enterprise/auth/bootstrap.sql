-- Infrastructure-only, run once as the PostgreSQL administrator. Schema/role/grant
-- DDL is an explicit Reinhardt Query exception: its schema API cannot provision
-- PostgreSQL principals or default privileges. No application data is written.
-- Provision strong LOGIN credentials through the secret manager separately.
CREATE ROLE aidash_auth_owner NOLOGIN;
CREATE ROLE aidash_auth_app NOLOGIN;
CREATE SCHEMA aidash_auth AUTHORIZATION aidash_auth_owner;
REVOKE ALL ON SCHEMA aidash_auth FROM PUBLIC;
GRANT USAGE ON SCHEMA aidash_auth TO aidash_auth_app;
ALTER ROLE aidash_auth_owner SET search_path = aidash_auth, pg_catalog;
ALTER ROLE aidash_auth_app SET search_path = aidash_auth, pg_catalog;
ALTER DEFAULT PRIVILEGES FOR ROLE aidash_auth_owner IN SCHEMA aidash_auth
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO aidash_auth_app;
