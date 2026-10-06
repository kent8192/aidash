ALTER SEQUENCE public.run_inputs_seq_seq OWNED BY NONE;

ALTER SEQUENCE public.run_activations_generation_seq OWNED BY NONE;

ALTER TABLE public.generation_history ALTER COLUMN sequence DROP IDENTITY;

ALTER TABLE public.events ALTER COLUMN sequence DROP IDENTITY;
