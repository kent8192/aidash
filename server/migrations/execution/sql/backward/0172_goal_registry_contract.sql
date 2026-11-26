DO $$
DECLARE definition text;
BEGIN
 definition := pg_get_functiondef('public.aidash_descriptor_before_goal_0172(jsonb)'::regprocedure);
 EXECUTE replace(definition, 'FUNCTION public.aidash_descriptor_before_goal_0172(', 'FUNCTION public.aidash_descriptor_is_valid(');
 definition := pg_get_functiondef('public.aidash_agent_before_goal_0172(jsonb)'::regprocedure);
 EXECUTE replace(definition, 'FUNCTION public.aidash_agent_before_goal_0172(', 'FUNCTION public.aidash_agent_bindings_is_valid(');
END $$;
DROP FUNCTION public.aidash_descriptor_before_goal_0172(jsonb);
DROP FUNCTION public.aidash_agent_before_goal_0172(jsonb);
