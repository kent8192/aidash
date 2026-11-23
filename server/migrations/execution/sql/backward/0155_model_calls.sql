DROP TRIGGER aidash_inference_usage_owner ON public.inference_usage;
DROP TRIGGER aidash_inference_attempt_owner ON public.inference_attempts;
DROP FUNCTION public.aidash_inference_owner();
CREATE TRIGGER aidash_inference_attempt_stint BEFORE INSERT ON public.inference_attempts
    FOR EACH ROW EXECUTE FUNCTION public.aidash_inference_attempt_stint();

-- A rollback requires external model-call usage to be cleared explicitly.
ALTER TABLE public.inference_usage ALTER COLUMN run_id SET NOT NULL;
