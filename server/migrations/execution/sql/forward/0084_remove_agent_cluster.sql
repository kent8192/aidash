-- JSON data transformation is not expressible by native migration operations.
SET LOCAL aidash.scope = 'deployment';
UPDATE public.generation_policies SET spec = jsonb_set(spec, '{template,config}', (spec#>'{template,config}') - 'cluster')
 WHERE spec#>'{template,config}' ? 'cluster';
UPDATE public.generation_policy_history SET spec = jsonb_set(spec, '{template,config}', (spec#>'{template,config}') - 'cluster')
 WHERE spec#>'{template,config}' ? 'cluster';
UPDATE public.generation_requests SET definition = jsonb_set(definition, '{config}', (definition->'config') - 'cluster')
 WHERE definition->>'kind' = 'agent' AND definition->'config' ? 'cluster';
