\connect template1
CREATE EXTENSION pg_jsonschema WITH SCHEMA public;
\connect aidash_a
CREATE EXTENSION pg_jsonschema WITH SCHEMA public;
CREATE DATABASE aidash_b TEMPLATE template1;
CREATE DATABASE aidash_test TEMPLATE template1;
