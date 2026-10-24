-- Run once as the PostgreSQL administrator. Supply strong login passwords
-- through your secret manager separately; never use Cloud/Auth service roles.
-- CREATE SCHEMA/ROLE, GRANT and ALTER DEFAULT PRIVILEGES are unsupported by
-- Reinhardt Query, so this infrastructure-only DDL is an explicit exception.
CREATE ROLE marketplace_owner NOLOGIN;
CREATE ROLE marketplace_app NOLOGIN;
CREATE SCHEMA marketplace AUTHORIZATION marketplace_owner;
REVOKE ALL ON SCHEMA marketplace FROM PUBLIC;
GRANT USAGE ON SCHEMA marketplace TO marketplace_app;
ALTER ROLE marketplace_owner SET search_path = marketplace, pg_catalog;
ALTER ROLE marketplace_app SET search_path = marketplace, pg_catalog;
ALTER DEFAULT PRIVILEGES FOR ROLE marketplace_owner IN SCHEMA marketplace
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO marketplace_app;
