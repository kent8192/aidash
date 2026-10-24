-- REVOKE and catalog-driven role iteration have no Reinhardt Query operation.
-- Keep the native migration history read-only for every non-owner role.
DO $$ DECLARE grantee text; BEGIN
FOR grantee IN
 SELECT DISTINCT CASE WHEN a.grantee=0 THEN 'PUBLIC' ELSE quote_ident(g.rolname) END
 FROM pg_class c CROSS JOIN LATERAL aclexplode(c.relacl) a
 LEFT JOIN pg_roles g ON g.oid=a.grantee
 WHERE c.oid='reinhardt_migrations'::regclass AND a.grantee<>c.relowner
 AND a.privilege_type<>'SELECT'
LOOP
 EXECUTE format('REVOKE INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER ON reinhardt_migrations FROM %s',grantee);
END LOOP;
END $$;
