SET LOCAL aidash.scope = 'deployment';
UPDATE "semantic_indexes" SET "spec" = jsonb_set("spec", '{embedding,provider}', '"openai_compatible"') WHERE ("spec" #>> '{embedding,provider}' IN ('openrouter', 'openai')) AND (COALESCE("spec" #> '{embedding,provider_credential}', 'null'::jsonb) = 'null'::jsonb);
