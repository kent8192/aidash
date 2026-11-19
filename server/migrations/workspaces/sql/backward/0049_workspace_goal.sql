-- A Workspace restored from a Thread takes its title as its goal (#1008).
SET LOCAL aidash.scope = 'deployment';
UPDATE public.workspaces SET goal = title;
ALTER TABLE public.workspaces ALTER COLUMN goal DROP DEFAULT;
