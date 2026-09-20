CREATE TABLE delegations (
    task_id uuid PRIMARY KEY REFERENCES tasks(id),
    node_id text NOT NULL,
    agent_id text NOT NULL,
    agent_version text NOT NULL,
    delivered boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE peer_events (
    node_id text NOT NULL,
    event_id uuid NOT NULL,
    PRIMARY KEY(node_id,event_id)
);
