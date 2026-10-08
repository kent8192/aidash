CREATE TRIGGER memory_model_operations_atomic BEFORE INSERT OR UPDATE OR DELETE ON memory_model_operations FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();
CREATE TRIGGER memory_model_attempts_atomic BEFORE INSERT OR UPDATE OR DELETE ON memory_model_attempts FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();
CREATE TRIGGER memory_publications_atomic BEFORE INSERT OR UPDATE OR DELETE ON memory_publications FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();
CREATE TRIGGER memory_task_participants_atomic BEFORE INSERT OR UPDATE OR DELETE ON memory_task_participants FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();
