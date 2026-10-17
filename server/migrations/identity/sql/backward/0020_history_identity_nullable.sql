-- The restored schema cannot represent history that concerns no External
-- Identity: Tenant creation through the Deployment Administrator bearer token
-- and Invitation events, whose Invitations and references this reversal drops
-- anyway. Those rows go with them, so restoring the constraint never fails
-- partway through a downgrade.
DELETE FROM public.dashboard_administration_history WHERE identity_id IS NULL;
ALTER TABLE public.dashboard_administration_history ALTER COLUMN identity_id SET NOT NULL;
