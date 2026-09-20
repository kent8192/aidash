-- Preserve read authority for structured records copied into durable run journals.
CREATE TABLE authorization_run_reads (
    run_id uuid NOT NULL REFERENCES runs(id),
    workspace_id uuid NOT NULL REFERENCES authorization_workspaces(workspace_id),
    resource_kind text NOT NULL CHECK (resource_kind IN ('task','artifact','message','run','conversation','generation')),
    resource_id uuid NOT NULL,
    PRIMARY KEY (run_id,resource_kind,resource_id)
);
-- Older journals did not track source membership. Conservatively retain every
-- existing workspace record as a dependency instead of assuming an empty set.
INSERT INTO authorization_run_reads(run_id,workspace_id,resource_kind,resource_id)
SELECT e.run_id,e.workspace_id,s.kind,s.id
FROM authorization_execution e
JOIN runs r ON r.id=e.run_id
JOIN LATERAL (
    SELECT 'task' AS kind,t.id FROM tasks t WHERE t.workspace_id=e.workspace_id
    UNION ALL SELECT 'artifact',a.id FROM artifacts a WHERE a.workspace_id=e.workspace_id
    UNION ALL SELECT 'message',m.id FROM messages m WHERE m.workspace_id=e.workspace_id
    UNION ALL SELECT 'conversation',c.id FROM conversations c WHERE c.workspace_id=e.workspace_id
    UNION ALL SELECT 'generation',g.id FROM generation_requests g WHERE g.workspace_id=e.workspace_id
    UNION ALL SELECT 'run',other.id FROM runs other WHERE other.workspace_id=e.workspace_id AND other.id<>e.run_id
) s ON true
WHERE r.phase<>'READY' OR r.context<>'{}'::jsonb OR r.pending<>'{}'::jsonb
ON CONFLICT DO NOTHING;
