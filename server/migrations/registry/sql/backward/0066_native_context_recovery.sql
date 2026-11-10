-- Definitions using the new policy cannot satisfy the earlier validator.
DO $$ BEGIN
 IF EXISTS (
  SELECT 1 FROM public.registry WHERE kind = 'agent'
  AND (metadata #> '{config,context_policy}' ?| ARRAY['offload_target','protected_tail_fraction','reread_immunity_steps','reread_summary_threshold']
   OR jsonb_typeof(metadata #> '{config,context_policy,summary}') = 'object')
 ) THEN
  RAISE EXCEPTION 'registry 0066 cannot be reversed: definitions use native context recovery' USING ERRCODE = '55000';
 END IF;
END $$;
DO $$ DECLARE original text; BEGIN
 original := pg_get_functiondef('public.aidash_agent_before_native_recovery_0066(jsonb)'::regprocedure);
 EXECUTE replace(original, 'FUNCTION public.aidash_agent_before_native_recovery_0066(', 'FUNCTION public.aidash_agent_bindings_is_valid(');
END $$;
DROP FUNCTION public.aidash_agent_before_native_recovery_0066(jsonb);
