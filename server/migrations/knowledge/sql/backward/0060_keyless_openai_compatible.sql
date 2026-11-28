SET LOCAL aidash.scope = 'deployment';
UPDATE "semantic_indexes" SET "spec" = jsonb_set("spec", '{embedding,provider}', '"openai"') WHERE ("spec" #>> '{embedding,provider}' IN ('openai_compatible')) AND (COALESCE("spec" #> '{embedding,provider_credential}', 'null'::jsonb) = 'null'::jsonb);
