"""Regression coverage for dump classification used by rebaselining."""

import importlib.util
from pathlib import Path
import unittest
import tempfile

SPEC = importlib.util.spec_from_file_location(
    "rebaseline", Path(__file__).with_name("rebaseline-migrations.py")
)
rebaseline = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(rebaseline)


def entry(name, kind, body):
    return f"-- Name: {name}; Type: {kind}; Schema: public; Owner: aidash_owner\n--\n\n{body}\n\n--\n"


class DumpPartitionTests(unittest.TestCase):
    def test_constraints_follow_target_ownership(self):
        owners = {"a": "identity", "b": "registry", "c": "identity"}
        dump = (
            entry("a", "TABLE", "CREATE TABLE public.a (id int);")
            + entry(
                "a a_b",
                "FK CONSTRAINT",
                "ALTER TABLE ONLY public.a ADD CONSTRAINT a_b FOREIGN KEY (id) REFERENCES public.b(id);",
            )
            + entry(
                "a a_c",
                "FK CONSTRAINT",
                "ALTER TABLE ONLY public.a ADD CONSTRAINT a_c FOREIGN KEY (id) REFERENCES public.c(id);",
            )
        )
        early, late = rebaseline.assign_schema(dump, owners, {})["identity"]
        self.assertTrue(any("a_c" in sql for sql in early))
        self.assertTrue(any("a_b" in sql for sql in late))
        self.assertFalse(any("a_b" in sql for sql in early))

    def test_comments_and_function_grants_follow_owned_objects(self):
        dump = entry(
            "TABLE a", "COMMENT", "COMMENT ON TABLE public.a IS 'kept';"
        ) + entry(
            "FUNCTION validate(text)",
            "ACL",
            "GRANT EXECUTE ON FUNCTION public.validate(text) TO reader;",
        )
        early, late = rebaseline.assign_schema(
            dump, {"a": "identity"}, {"validate": "identity"}
        )["identity"]
        self.assertEqual(early, [])
        self.assertTrue(any("COMMENT ON TABLE" in sql for sql in late))
        self.assertTrue(any("GRANT EXECUTE" in sql for sql in late))

    def test_unknown_table_fails_closed(self):
        with self.assertRaisesRegex(ValueError, "unknown owner"):
            rebaseline.assign_schema(
                entry(
                    "unclassified",
                    "TABLE",
                    "CREATE TABLE public.unclassified (id int);",
                ),
                {},
                {},
            )

    def test_default_grant_and_fixed_owner_are_not_replayed(self):
        owners = {"a": "identity"}
        dump = entry(
            "a",
            "TABLE",
            "CREATE TABLE public.a (id int);\nALTER TABLE public.a OWNER TO aidash_owner;",
        ) + entry(
            "TABLE a",
            "ACL",
            "GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE public.a TO aidash_server;",
        )
        early, late = rebaseline.assign_schema(dump, owners, {})["identity"]
        self.assertNotIn("OWNER TO", "".join(early))
        self.assertEqual(late, [])

    def test_table_owner_can_be_inferred_from_the_creating_migration(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            classification = root / "server/src/database/classification.rs"
            classification.parent.mkdir(parents=True)
            classification.write_text('table("identity", "tenants", DeploymentWide)')
            migration = root / "server/migrations/knowledge/0053_proofs.rs"
            migration.parent.mkdir(parents=True)
            migration.write_text(
                'Operation::CreateTable { name: "memory_journal_proofs".into() }'
            )
            owners = rebaseline.resolve_table_owners(root)
            self.assertEqual(owners["tenants"], "identity")
            self.assertEqual(owners["memory_journal_proofs"], "knowledge")

    def test_unordered_named_state_constraints_are_stable(self):
        start = "\t\t\tconstraints: vec![\n"
        end = "\t\t\t],"

        def fk(name):
            return f'\t\t\t\tConstraint::ForeignKey {{\n\t\t\t\t\tname: "{name}".to_string(),\n\t\t\t\t}},\n'

        first = start + fk("z") + fk("a") + end
        second = start + fk("a") + fk("z") + end
        canonical = rebaseline.canonicalize_state_constraints(first)
        self.assertEqual(canonical, rebaseline.canonicalize_state_constraints(second))
        self.assertEqual(
            rebaseline.canonicalize_state_constraints(canonical), canonical
        )
        wrapped = first.replace(
            'name: "z".to_string()', 'name :\n            "z"\n            .to_string()'
        )
        self.assertEqual(rebaseline.canonicalize_state_constraints(wrapped), canonical)

    def test_control_whitespace_in_sql_literals_is_escaped_without_changing_characters(
        self,
    ):
        sql = "CHECK (length(btrim(value, '\t\n\x0b\x0c\r \u0085')) > 0)"
        sql = sql.replace("'))", "'::text))")
        result = rebaseline.escape_whitespace_literals(sql)
        self.assertIn(r"E'\u0009\u000a\u000b\u000c\u000d\u0020\u0085'::text", result)
        self.assertNotIn("\r", result)
        self.assertNotIn("\n", result)

    def test_data_headers_are_parsed(self):
        dump = "-- Data for Name: atomic_gate; Type: TABLE DATA; Schema: public; Owner: aidash_owner\n--\n\nINSERT INTO public.atomic_gate VALUES (true);\n"
        rows = list(rebaseline.dump_entries(dump))
        self.assertEqual(rows[0][:3], ("atomic_gate", "TABLE DATA", "public"))
        self.assertIn("INSERT INTO", rows[0][3])


if __name__ == "__main__":
    unittest.main()
