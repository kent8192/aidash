-- PostgreSQL function/trigger DDL exception: SeaQuery has no procedural builder.
CREATE FUNCTION aidash_request_activation(target UUID, cause TEXT) RETURNS VOID
LANGUAGE SQL AS $$
    INSERT INTO run_activations (run_id, run_revision, reason)
    SELECT id, revision, cause FROM runs WHERE id = target;
$$;

CREATE FUNCTION aidash_activation_trigger() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME = 'runs' THEN
        IF TG_OP = 'UPDATE' THEN
            -- Heartbeats and lease acquisition are not new scheduling intent.
            IF NEW.phase IS NOT DISTINCT FROM OLD.phase
               AND NEW.control IS NOT DISTINCT FROM OLD.control
               AND (NEW.pending - 'lease_recovered') IS NOT DISTINCT FROM (OLD.pending - 'lease_recovered')
               AND NEW.step IS NOT DISTINCT FROM OLD.step
               AND NOT (OLD.lease_owner IS NOT NULL AND NEW.lease_owner IS NULL) THEN
                RETURN NEW;
            END IF;
        END IF;
        PERFORM aidash_request_activation(NEW.id, 'run_transition');
        IF NEW.phase IN ('COMPLETED', 'FAILED', 'CANCELLED') THEN
            -- Only the head's completion releases admission. Notify one successor,
            -- skipping terminal gaps while preserving paused predecessors.
            INSERT INTO run_activations (run_id, run_revision, reason)
            SELECT r.id, r.revision, 'ordering_release' FROM core_runs mine
              JOIN core_runs next ON next.area_id = mine.area_id AND next.generation = mine.generation
              JOIN runs r ON r.id = next.run_id
             WHERE mine.run_id = NEW.id AND next.sequence > mine.sequence
               AND r.phase NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
               AND NOT EXISTS (
                   SELECT 1 FROM core_runs earlier JOIN runs pending ON pending.id = earlier.run_id
                    WHERE earlier.area_id = mine.area_id AND earlier.generation = mine.generation
                      AND earlier.sequence < mine.sequence
                      AND pending.phase NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
               )
             ORDER BY next.sequence LIMIT 1;
        END IF;
    ELSIF TG_TABLE_NAME = 'tasks' THEN
        IF NEW.status IS DISTINCT FROM OLD.status THEN
            INSERT INTO run_activations (run_id, run_revision, reason)
            SELECT r.id, r.revision, 'dependency_release' FROM runs r
              JOIN tasks t ON t.id = r.task_id
             WHERE NEW.id = ANY(t.dependencies)
               AND r.phase NOT IN ('COMPLETED', 'FAILED', 'CANCELLED');
        END IF;
    ELSIF TG_TABLE_NAME = 'run_inputs' THEN
        PERFORM aidash_request_activation(NEW.run_id, 'input');
    ELSIF TG_TABLE_NAME = 'human_requests' THEN
        IF NEW.response IS DISTINCT FROM OLD.response THEN
            PERFORM aidash_request_activation(NEW.run_id, 'human_response');
        END IF;
    ELSIF TG_TABLE_NAME = 'core_records' THEN
        INSERT INTO run_activations (run_id, run_revision, reason)
        SELECT id, revision, 'approval' FROM runs
         WHERE pending ->> 'core_approval_id' = NEW.id::TEXT;
    ELSIF TG_TABLE_NAME = 'core_runs' THEN
        PERFORM aidash_request_activation(NEW.run_id, 'core_admission');
    ELSIF TG_TABLE_NAME = 'core_areas' THEN
        IF NEW.state IS DISTINCT FROM OLD.state THEN
            INSERT INTO run_activations (run_id, run_revision, reason)
            SELECT r.id, r.revision, 'area_release' FROM runs r JOIN core_runs c ON c.run_id = r.id
             WHERE c.area_id = NEW.id AND r.phase NOT IN ('COMPLETED', 'FAILED', 'CANCELLED');
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER aidash_activation AFTER INSERT OR UPDATE ON runs FOR EACH ROW EXECUTE FUNCTION aidash_activation_trigger();
CREATE TRIGGER aidash_activation AFTER INSERT ON run_inputs FOR EACH ROW EXECUTE FUNCTION aidash_activation_trigger();
CREATE TRIGGER aidash_activation AFTER UPDATE OF response ON human_requests FOR EACH ROW EXECUTE FUNCTION aidash_activation_trigger();
CREATE TRIGGER aidash_activation AFTER INSERT OR UPDATE OF state, expires_at ON core_records FOR EACH ROW EXECUTE FUNCTION aidash_activation_trigger();
CREATE TRIGGER aidash_activation AFTER INSERT OR UPDATE OF initialized ON core_runs FOR EACH ROW EXECUTE FUNCTION aidash_activation_trigger();
CREATE TRIGGER aidash_activation AFTER UPDATE OF state ON core_areas FOR EACH ROW EXECUTE FUNCTION aidash_activation_trigger();
-- Gate closure preserves existing obligations: publication retries and consumer
-- redelivery resume after release. Do not scan/activate unrelated Runs here.
CREATE TRIGGER aidash_activation AFTER UPDATE OF status ON tasks FOR EACH ROW EXECUTE FUNCTION aidash_activation_trigger();
